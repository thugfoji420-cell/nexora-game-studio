#!/usr/bin/env python3
"""
Nexora 3D Mesh Processor - AAA Game-Ready Blender Headless Script

This script runs inside Blender headless mode to process 3D assets through
the Nexora AAA pipeline stages: importing, analyzing, cleaning, repairing,
scale/origin normalization, optimizing, LOD generation, collision generation,
and comprehensive readiness validation.

Supported Profiles:
    generic, vehicle, character, environment, prop, weapon,
    building, vegetation, furniture, equipment
"""

import sys
import os
import json
import argparse
import traceback
import math
import bpy
import bmesh
from mathutils import Vector, Matrix
from pathlib import Path

# ============================================================================
# Configuration & Constants
# ============================================================================

QUALITY_TRIANGLE_TARGETS = {
    "master": None,  # No decimation
    "mobile_high": 50000,
    "mobile_balanced": 35000,
    "mobile_low": 20000,
}

MERGE_DISTANCE = 0.0001
DEGENERATE_AREA_THRESHOLD = 1e-8
MIN_COMPONENT_VERTICES = 10

# Real-world target dimensions (in meters) for scale normalization
PROFILE_TARGET_SCALES = {
    "vehicle": {"max_dim": 4.4, "ground_origin": True, "forward_axis": "Y"},
    "character": {"height": 1.8, "ground_origin": True, "forward_axis": "Z"},
    "weapon": {"length": 0.9, "ground_origin": False, "forward_axis": "Z"},
    "prop": {"max_dim": 1.2, "ground_origin": True, "forward_axis": "Z"},
    "equipment": {"max_dim": 0.8, "ground_origin": False, "forward_axis": "Z"},
    "furniture": {"height": 0.9, "ground_origin": True, "forward_axis": "Z"},
    "building": {"height": 8.0, "ground_origin": True, "forward_axis": "Z"},
    "vegetation": {"height": 4.0, "ground_origin": True, "forward_axis": "Z"},
    "environment": {"max_dim": 6.0, "ground_origin": True, "forward_axis": "Z"},
    "generic": {"max_dim": 2.0, "ground_origin": True, "forward_axis": "Z"},
}

# Category-specific processing configurations
CATEGORY_PROCESSING_CONFIG = {
    "vehicle": {
        "detect_wheels": True,
        "separate_wheels": True,
        "generate_collision": True,
        "collision_type": "convex_hull_per_part",
        "lod_ratios": [1.0, 0.5, 0.25],
        "require_ground_contact": True,
    },
    "character": {
        "detect_skeleton": True,
        "validate_rig": True,
        "generate_collision": True,
        "collision_type": "capsule_hull",
        "lod_ratios": [1.0, 0.5, 0.2],
        "require_t_pose": False,
    },
    "prop": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.5, 0.25],
        "center_pivot": True,
    },
    "weapon": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.6, 0.3],
        "align_grip_origin": True,
    },
    "building": {
        "generate_collision": True,
        "collision_type": "box_per_module",
        "lod_ratios": [1.0, 0.4, 0.15],
        "modular_pivots": True,
    },
    "vegetation": {
        "generate_collision": False,
        "collision_type": "none",
        "lod_ratios": [1.0, 0.5, 0.2],
        "billboard_lod2": True,
    },
    "environment": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.4, 0.15],
        "modular_pivots": True,
    },
    "furniture": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.5, 0.25],
        "ground_origin": True,
    },
    "equipment": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.5, 0.25],
    },
    "generic": {
        "generate_collision": True,
        "collision_type": "convex_hull",
        "lod_ratios": [1.0, 0.5, 0.25],
    },
}

# ============================================================================
# Utility Functions
# ============================================================================

def log(msg, level="INFO"):
    """Log message to stdout with level prefix."""
    print(f"[{level}] {msg}", flush=True)

def get_scene_mesh_objects():
    """Get all mesh objects in the scene."""
    return [obj for obj in bpy.context.scene.objects if obj.type == 'MESH']

def get_object_triangle_count(obj):
    """Get triangle count for a mesh object."""
    if obj.type != 'MESH':
        return 0
    mesh = obj.data
    if mesh.polygons:
        return sum(1 for _ in mesh.polygons)
    bm = bmesh.new()
    bm.from_mesh(mesh)
    bmesh.ops.triangulate(bm, faces=bm.faces)
    tri_count = len(bm.faces)
    bm.free()
    return tri_count

def get_scene_stats():
    """Get comprehensive scene statistics."""
    objects = get_scene_mesh_objects()
    total_verts = 0
    total_faces = 0
    total_tris = 0
    materials = set()
    
    for obj in objects:
        mesh = obj.data
        total_verts += len(mesh.vertices)
        total_faces += len(mesh.polygons)
        total_tris += get_object_triangle_count(obj)
        for slot in obj.material_slots:
            if slot.material:
                materials.add(slot.material.name)
    
    return {
        "object_count": len(objects),
        "vertex_count": total_verts,
        "face_count": total_faces,
        "triangle_count": total_tris,
        "material_count": len(materials),
    }

def write_json(path, data):
    """Write JSON with proper formatting."""
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(data, f, indent=2)

def export_objects(objects, output_path):
    """Export selected objects as GLB with standard Unity coordinates."""
    bpy.ops.object.select_all(action='DESELECT')
    for obj in objects:
        obj.select_set(True)
    bpy.ops.export_scene.gltf(
        filepath=output_path,
        export_format='GLB',
        use_selection=True,
        export_apply=True,
        export_yup=True,
    )

# ============================================================================
# Processing Stages
# ============================================================================

def stage_import(input_path, output_dir):
    """Stage 1: Import the source 3D file."""
    log(f"Importing {input_path}")
    ext = Path(input_path).suffix.lower()
    
    if ext in ('.glb', '.gltf'):
        bpy.ops.import_scene.gltf(filepath=input_path)
    elif ext == '.fbx':
        bpy.ops.import_scene.fbx(filepath=input_path)
    elif ext == '.obj':
        bpy.ops.import_scene.obj(filepath=input_path)
    elif ext == '.ply':
        bpy.ops.import_mesh.ply(filepath=input_path)
    else:
        raise ValueError(f"Unsupported format: {ext}")
    
    working_path = output_dir / "working.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(working_path))
    
    stats = get_scene_stats()
    log(f"Imported: {stats}")
    return stats

def stage_analyze(stats_before):
    """Stage 2: Analyze geometry manifoldness and topology."""
    log("Analyzing mesh topology...")
    stats_after = get_scene_stats()
    
    non_manifold_count = 0
    loose_verts = 0
    loose_edges = 0
    degenerate_count = 0
    
    for obj in get_scene_mesh_objects():
        bm = bmesh.new()
        bm.from_mesh(obj.data)
        for edge in bm.edges:
            if len(edge.link_faces) != 2:
                non_manifold_count += 1
            if len(edge.link_faces) == 0:
                loose_edges += 1
        for vert in bm.verts:
            if len(vert.link_edges) == 0:
                loose_verts += 1
        for face in bm.faces:
            if face.calc_area() < DEGENERATE_AREA_THRESHOLD:
                degenerate_count += 1
        bm.free()
    
    analysis = {
        "vertices_before": stats_before.get("vertex_count", 0),
        "vertices_after": stats_after.get("vertex_count", 0),
        "faces_before": stats_before.get("face_count", 0),
        "faces_after": stats_after.get("face_count", 0),
        "triangles_before": stats_before.get("triangle_count", 0),
        "triangles_after": stats_after.get("triangle_count", 0),
        "objects_before": stats_before.get("object_count", 0),
        "objects_after": stats_after.get("object_count", 0),
        "materials_before": stats_before.get("material_count", 0),
        "materials_after": stats_after.get("material_count", 0),
        "non_manifold_edges": non_manifold_count,
        "degenerate_faces": degenerate_count,
        "loose_vertices": loose_verts,
        "loose_edges": loose_edges,
        "warnings": [],
    }
    
    if non_manifold_count > 0:
        analysis["warnings"].append(f"Found {non_manifold_count} non-manifold edges")
    if degenerate_count > 0:
        analysis["warnings"].append(f"Found {degenerate_count} degenerate faces")
    
    log(f"Analysis complete: {analysis}")
    return analysis

def stage_clean():
    """Stage 3: Remove loose vertices, edges, and zero-area faces."""
    log("Cleaning mesh...")
    bpy.ops.object.mode_set(mode='OBJECT')
    
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='DESELECT')
        bpy.ops.mesh.select_loose()
        bpy.ops.mesh.delete(type='VERT')
        bpy.ops.object.mode_set(mode='OBJECT')
    
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.mode_set(mode='EDIT')
        bm = bmesh.from_edit_mesh(obj.data)
        degen = [f for f in bm.faces if f.calc_area() < DEGENERATE_AREA_THRESHOLD]
        if degen:
            bmesh.ops.delete(bm, geom=degen, context='FACES')
        bmesh.update_edit_mesh(obj.data)
        bpy.ops.object.mode_set(mode='OBJECT')
    
    stats = get_scene_stats()
    log(f"Clean complete: {stats}")
    return stats

def stage_normalize_scale_and_origin(profile="generic"):
    """Stage 4: Normalization - standard unit scale & ground origin."""
    log(f"Normalizing scale and origin for profile '{profile}'...")
    objects = get_scene_mesh_objects()
    if not objects:
        return {"scale_normalized": False, "origin_normalized": False}
    
    # Calculate global bounding box across all mesh objects
    min_co = Vector((float('inf'), float('inf'), float('inf')))
    max_co = Vector((float('-inf'), float('-inf'), float('-inf')))
    
    for obj in objects:
        for corner in obj.bound_box:
            world_co = obj.matrix_world @ Vector(corner)
            min_co.x = min(min_co.x, world_co.x)
            min_co.y = min(min_co.y, world_co.y)
            min_co.z = min(min_co.z, world_co.z)
            max_co.x = max(max_co.x, world_co.x)
            max_co.y = max(max_co.y, world_co.y)
            max_co.z = max(max_co.z, world_co.z)
    
    dim = max_co - min_co
    center = (min_co + max_co) * 0.5
    
    cfg = PROFILE_TARGET_SCALES.get(profile, PROFILE_TARGET_SCALES["generic"])
    scale_factor = 1.0
    
    if "height" in cfg and dim.z > 0.001:
        current_val = dim.z
        target_val = cfg["height"]
        if current_val < 0.1 or current_val > 50.0:
            scale_factor = target_val / current_val
    elif "max_dim" in cfg:
        current_val = max(dim.x, dim.y, dim.z)
        target_val = cfg["max_dim"]
        if current_val < 0.1 or current_val > 50.0:
            scale_factor = target_val / current_val
    
    # Apply scale factor if needed
    if abs(scale_factor - 1.0) > 0.05:
        log(f"Adjusting scale by factor {scale_factor:.3f} to match real-world {profile} metrics")
        for obj in objects:
            obj.scale *= scale_factor
            bpy.context.view_layer.objects.active = obj
            bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    
    # Recalculate bounding box for origin alignment
    min_co = Vector((float('inf'), float('inf'), float('inf')))
    max_co = Vector((float('-inf'), float('-inf'), float('-inf')))
    for obj in objects:
        for corner in obj.bound_box:
            world_co = obj.matrix_world @ Vector(corner)
            min_co.x = min(min_co.x, world_co.x)
            min_co.y = min(min_co.y, world_co.y)
            min_co.z = min(min_co.z, world_co.z)
            max_co.x = max(max_co.x, world_co.x)
            max_co.y = max(max_co.y, world_co.y)
            max_co.z = max(max_co.z, world_co.z)
    
    center_x = (min_co.x + max_co.x) * 0.5
    center_y = (min_co.y + max_co.y) * 0.5
    ground_z = min_co.z if cfg.get("ground_origin", True) else (min_co.z + max_co.z) * 0.5
    
    # Shift scene objects so origin is center X/Y, ground Z
    offset = Vector((-center_x, -center_y, -ground_z))
    for obj in objects:
        obj.location += offset
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.transform_apply(location=True, rotation=False, scale=False)
    
    log(f"Origin normalized: X/Y centered, Ground Z at 0.0 (offset: {offset})")
    return {
        "scale_normalized": True,
        "origin_normalized": True,
        "scale_factor": scale_factor,
        "ground_origin": cfg.get("ground_origin", True),
    }

def stage_repair():
    """Stage 5: Repair normals and apply transforms."""
    log("Repairing mesh normals and doubles...")
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
        
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.mesh.normals_make_consistent(inside=False)
        bpy.ops.mesh.remove_doubles(threshold=MERGE_DISTANCE)
        bpy.ops.object.mode_set(mode='OBJECT')
    
    stats = get_scene_stats()
    # Honest post-repair topology recount (grade the delivered asset,
    # not the raw generator output)
    non_man_after = 0
    for obj in get_scene_mesh_objects():
        bm_tmp = bmesh.new()
        bm_tmp.from_mesh(obj.data)
        non_man_after += sum(1 for e in bm_tmp.edges if len(e.link_faces) != 2)
        bm_tmp.free()
    stats["non_manifold_after"] = non_man_after
    log(f"Repair complete: {stats}")
    return stats

# ============================================================================
# PBR Material Readiness Stages (AAA standard)
# ============================================================================

def _find_basecolor_image(mat):
    """Return (image_node, image) feeding Principled Base Color, if any."""
    if not mat.use_nodes:
        return None, None
    bsdf = next((n for n in mat.node_tree.nodes if n.type == 'BSDF_PRINCIPLED'), None)
    if not bsdf:
        return None, None
    inp = bsdf.inputs.get('Base Color')
    if inp and inp.is_linked:
        node = inp.links[0].from_node
        if node.type == 'TEX_IMAGE' and node.image:
            return node, node.image
    return None, None


def _derive_normal_map(img, strength=0.55):
    """Sobel the albedo luminance into a tangent-space OpenGL normal map."""
    import numpy as np
    w, h = img.size
    if w == 0 or h == 0:
        return None
    px = np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4)
    lum = 0.2126 * px[:, :, 0] + 0.7152 * px[:, :, 1] + 0.0722 * px[:, :, 2]
    lum = (lum - lum.min()) / max(1e-6, (lum.max() - lum.min()))
    p = np.pad(lum, 1, mode='edge')
    gx = (p[:-2, 2:] + 2 * p[1:-1, 2:] + p[2:, 2:]) - (p[:-2, :-2] + 2 * p[1:-1, :-2] + p[2:, :-2])
    gy = (p[2:, :-2] + 2 * p[2:, 1:-1] + p[2:, 2:]) - (p[:-2, :-2] + 2 * p[:-2, 1:-1] + p[:-2, 2:])
    nx, ny, nz = -gx * strength * 8.0, -gy * strength * 8.0, np.ones_like(lum)
    ln = np.sqrt(nx * nx + ny * ny + nz * nz)
    out = np.zeros((h, w, 4), dtype=np.float32)
    out[:, :, 0] = nx / ln * 0.5 + 0.5
    out[:, :, 1] = ny / ln * 0.5 + 0.5
    out[:, :, 2] = nz / ln * 0.5 + 0.5
    out[:, :, 3] = 1.0
    nrm = bpy.data.images.new("Nexora_Normal", width=w, height=h, alpha=False)
    nrm.pixels = out.ravel().tolist()
    return nrm


def _derive_roughness_map(img):
    """Roughness from inverted albedo luminance, mean-normalized around 0.5."""
    import numpy as np
    w, h = img.size
    if w == 0 or h == 0:
        return None
    px = np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4)
    lum = 0.2126 * px[:, :, 0] + 0.7152 * px[:, :, 1] + 0.0722 * px[:, :, 2]
    lum = (lum - lum.min()) / max(1e-6, (lum.max() - lum.min()))
    rough = np.clip(1.0 - lum, 0.05, 1.0)
    rough = np.clip(0.5 + (rough - rough.mean()), 0.05, 1.0)
    out = np.zeros((h, w, 4), dtype=np.float32)
    out[:, :, 0] = out[:, :, 1] = out[:, :, 2] = rough
    out[:, :, 3] = 1.0
    rgh = bpy.data.images.new("Nexora_Roughness", width=w, height=h, alpha=False)
    rgh.pixels = out.ravel().tolist()
    return rgh


def _try_bake_ao(objs, img_size=1024, samples=32):
    """Cycles AO bake into a new image. Best-effort: returns None on failure."""
    try:
        scene = bpy.context.scene
        scene.render.engine = 'CYCLES'
        scene.cycles.samples = samples
        try:
            scene.cycles.device = 'CPU'
        except Exception:
            pass
        ao_img = bpy.data.images.new("Nexora_AO", width=img_size, height=img_size, alpha=False)
        for obj in objs:
            obj.select_set(True)
        bpy.context.view_layer.objects.active = objs[0]
        for obj in objs:
            if not obj.data.uv_layers:
                return None
        bpy.ops.object.bake(type='AO', use_clear=True, margin=8, pass_filter={'COLOR'})
        for obj in objs:
            for slot in obj.material_slots:
                mat = slot.material
                if not (mat and mat.use_nodes):
                    continue
                tex = mat.node_tree.nodes.new('ShaderNodeTexImage')
                tex.image = ao_img
                tex.location = (-600, -500)
        return ao_img
    except Exception as exc:
        log(f"AO bake skipped: {exc}", "WARN")
        return None


def stage_pbr_materials(segmentation=None):
    """Stage: enforce dielectric PBR values and derive normal/roughness maps.

    When semantic segmentation produced class materials, this is a light
    finishing pass only: per-class roughness/metallic/clearcoat values are
    already set, the global luminance-derived roughness map is intentionally
    NOT applied (it makes glass/trim matte), and only the paint class keeps
    the derived detail normal map."""
    log("Finishing PBR materials...")
    seg_classes = (segmentation or {}).get("classes", []) if segmentation else []
    segmented = bool(seg_classes)
    stats = {"materials_processed": 0, "normal_maps_created": 0,
             "roughness_maps_created": 0, "ao_baked": False,
             "metallic_clamped": 0, "roughness_clamped": 0,
             "segmented": segmented, "segmentation_classes": seg_classes}
    if segmented:
        # Class params are semantically set (glass glossy, rubber matte,
        # chrome metallic, paint clearcoat) - verify only, never clamp them.
        stats["normal_maps_created"] = (segmentation or {}).get("normal_maps_created", 0)
        for mat in bpy.data.materials:
            if not mat.name.startswith("Nexora_"):
                continue
            if not mat.use_nodes:
                mat.use_nodes = True
            bsdf = next((n for n in mat.node_tree.nodes if n.type == 'BSDF_PRINCIPLED'), None)
            if bsdf is None:
                continue
            stats["materials_processed"] += 1
        stats["sanity_verified"] = stats["materials_processed"] > 0
        log(f"PBR finish (segmented, params preserved) complete: {stats}")
        return stats
    for mat in bpy.data.materials:
        if not mat.use_nodes:
            mat.use_nodes = True
        nt = mat.node_tree
        bsdf = next((n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED'), None)
        if not bsdf:
            continue
        met = bsdf.inputs.get('Metallic')
        rgh = bsdf.inputs.get('Roughness')
        if met and not met.is_linked and met.default_value > 0.15:
            met.default_value = 0.0
            stats["metallic_clamped"] += 1
        if rgh and not rgh.is_linked and rgh.default_value < 0.45:
            rgh.default_value = 0.6
            stats["roughness_clamped"] += 1
        _tex_node, image = _find_basecolor_image(mat)
        if image is None:
            continue
        if any(n.type == 'NORMAL_MAP' for n in nt.nodes):
            continue  # already has a normal map
        normal_img = _derive_normal_map(image)
        rough_img = _derive_roughness_map(image)
        has_ao = any(n.type == 'TEX_IMAGE' and n.image and n.image.name.startswith('Nexora_AO') for n in nt.nodes)
        if not has_ao:
            ao_img = _try_bake_ao(get_scene_mesh_objects())
            if ao_img:
                # glTF exporters carry AO through the Principled Occlusion socket;
                # newer Blender builds removed that socket, so only count AO as
                # baked when it is actually wired into the shader.
                occl = bsdf.inputs.get('Occlusion')
                if occl is not None:
                    ao_tex = next((n for n in nt.nodes if n.type == 'TEX_IMAGE' and n.image and n.image.name.startswith('Nexora_AO')), None)
                    if ao_tex is not None:
                        nt.links.new(ao_tex.outputs['Color'], occl)
                        ao_img.pack()
                        stats["ao_baked"] = True
        if normal_img:
            nm_node = nt.nodes.new('ShaderNodeNormalMap')
            nm_node.location = (-500, -300)
            nm_node.inputs['Strength'].default_value = 0.6
            tex = nt.nodes.new('ShaderNodeTexImage')
            tex.image = normal_img
            tex.location = (-800, -300)
            nt.links.new(tex.outputs['Color'], nm_node.inputs['Color'])
            nt.links.new(nm_node.outputs['Normal'], bsdf.inputs['Normal'])
            normal_img.pack()
            stats["normal_maps_created"] += 1
        if rough_img:
            tex = nt.nodes.new('ShaderNodeTexImage')
            tex.image = rough_img
            tex.location = (-800, -600)
            nt.links.new(tex.outputs['Color'], rgh)
            rough_img.pack()
            stats["roughness_maps_created"] += 1
        stats["materials_processed"] += 1
    log(f"PBR upgrade complete: {stats}")
    return stats



# ============================================================================
# AAA Material Segmentation, Shading & Baked Maps
# ============================================================================

VEHICLE_CLASS_PARAMS = {
    "paint":  {"metallic": 0.0, "roughness": 0.38, "coat_weight": 1.0, "coat_roughness": 0.06, "normal_strength": 0.35},
    "glass":  {"metallic": 0.0, "roughness": 0.06, "coat_weight": 0.0, "coat_roughness": 0.03, "normal_strength": 0.0},
    "rubber": {"metallic": 0.0, "roughness": 0.92, "coat_weight": 0.0, "coat_roughness": 0.50, "normal_strength": 0.0},
    "chrome": {"metallic": 1.0, "roughness": 0.18, "coat_weight": 0.0, "coat_roughness": 0.10, "normal_strength": 0.0},
}


def _find_dominant_albedo():
    """Return (material, tex_node, image) for the largest albedo texture actually
    assigned to scene mesh objects, or (None, None, None)."""
    used_mats = set()
    for obj in get_scene_mesh_objects():
        for slot in obj.material_slots:
            if slot.material:
                used_mats.add(slot.material)
    best = (None, None, None)
    best_area = 0
    for mat in used_mats:
        node, image = _find_basecolor_image(mat)
        if image is not None and image.size[0] > 0:
            area = image.size[0] * image.size[1]
            if area > best_area:
                best = (mat, node, image)
                best_area = area
    return best


def _sample_face_albedo(obj, arr):
    """Per-face (area, r, g, b, z_world) sampled from albedo at the UV centroid."""
    import numpy as np
    mesh = obj.data
    if not mesh.uv_layers or len(mesh.polygons) == 0:
        return None
    uv = mesh.uv_layers[0].data
    h, w = arr.shape[:2]
    rows = []
    mw = obj.matrix_world
    verts = mesh.vertices
    for poly in mesh.polygons:
        loops = range(poly.loop_start, poly.loop_start + poly.loop_total)
        u = 0.0
        v = 0.0
        n = 0
        for li in loops:
            u += uv[li].uv[0]
            v += uv[li].uv[1]
            n += 1
        u = max(0.0, min(1.0, u / max(1, n)))
        v = max(0.0, min(1.0, v / max(1, n)))
        x = min(w - 1, int(u * w))
        y = min(h - 1, int((1.0 - v) * (h - 1)))
        r, g, b = (float(arr[y, x][0]), float(arr[y, x][1]), float(arr[y, x][2]))
        zc = 0.0
        for vi in poly.vertices:
            zc += (mw @ verts[vi].co).z
        zc /= len(poly.vertices)
        rows.append((float(poly.area), r, g, b, zc))
    return rows


def _classify_vehicle_faces(rows, bounds_z):
    """Assign each face to paint/glass/rubber/chrome using relative albedo
    thresholds + height heuristics. Returns list of class names per face."""
    import numpy as np
    import colorsys
    n = len(rows)
    area = np.array([r[0] for r in rows], dtype=np.float64)
    rgb = np.array([[r[1], r[2], r[3]] for r in rows], dtype=np.float64)
    z = np.array([r[4] for r in rows], dtype=np.float64)
    lum = 0.2126 * rgb[:, 0] + 0.7152 * rgb[:, 1] + 0.0722 * rgb[:, 2]
    cmax = rgb.max(axis=1)
    cmin = rgb.min(axis=1)
    sat = np.where(cmax > 1e-5, (cmax - cmin) / np.maximum(cmax, 1e-5), 0.0)
    zmin, zmax = bounds_z
    zn = (z - zmin) / max(1e-6, (zmax - zmin))

    lum_p85 = float(np.percentile(lum, 85))
    dark = lum < max(0.15, 0.35 * lum_p85)
    # Dominant (paint) color stats from non-dark faces
    nd = ~dark
    if not nd.any():
        nd = np.ones(n, dtype=bool)
        dark = np.zeros(n, dtype=bool)
    paint_sat = float(np.median(sat[nd]))
    paint_lum = float(np.median(lum[nd]))
    # Rescue shadowed body panels: a dark but paint-hued saturated face is a
    # shadow baked into the albedo, not glass.
    hue_rescue = np.zeros(n, dtype=bool)
    if paint_sat > 0.30:
        import colorsys as _cs
        med_rgb = np.median(rgb[nd], axis=0)
        ph, _ps, _pv = _cs.rgb_to_hsv(*np.clip(med_rgb, 0, 1))
        fh = np.array([_cs.rgb_to_hsv(min(1.0, max(0.0, r)), min(1.0, max(0.0, g)), min(1.0, max(0.0, b)))[0]
                       for r, g, b in rgb])
        dh = np.abs(fh - ph)
        dh = np.minimum(dh, 1.0 - dh)
        hue_rescue = dark & (sat > 0.30) & (dh < 0.06)
        dark = dark & ~hue_rescue
    # Chrome/trim only makes sense when the paint itself is colorful:
    bright_desat = (~dark) & (sat < 0.15) & (lum > max(0.72, paint_lum * 1.05))
    chrome = bright_desat & (paint_sat > 0.30) & (bright_desat.sum() < 0.5 * n)
    rubber = dark & (zn < 0.42)
    glass = dark & (zn >= 0.42) & (zn <= 0.88)
    paint = ~(rubber | glass | chrome)
    classes = []
    for i in range(n):
        if rubber[i]:
            classes.append("rubber")
        elif glass[i]:
            classes.append("glass")
        elif chrome[i]:
            classes.append("chrome")
        else:
            classes.append("paint")
    return classes


def _build_class_material(base_mat, base_image, cls, normal_image=None):
    """Create a PBR material for a semantic class, sharing the albedo texture."""
    params = VEHICLE_CLASS_PARAMS[cls]
    mat = bpy.data.materials.new(f"Nexora_{cls.capitalize()}")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    if bsdf is None:
        bsdf = next(n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED')
    if base_image is not None:
        tex = nt.nodes.new('ShaderNodeTexImage')
        tex.image = base_image
        tex.location = (-800, 0)
        nt.links.new(tex.outputs['Color'], bsdf.inputs['Base Color'])
        tex.interpolation = 'Smart'
    bsdf.inputs['Metallic'].default_value = params["metallic"]
    bsdf.inputs['Roughness'].default_value = params["roughness"]
    coat_w = bsdf.inputs.get('Coat Weight')
    coat_r = bsdf.inputs.get('Coat Roughness')
    if coat_w is not None and params["coat_weight"] > 0:
        coat_w.default_value = params["coat_weight"]
        if coat_r is not None:
            coat_r.default_value = params["coat_roughness"]
    if params["normal_strength"] > 0 and normal_image is not None:
        tex = nt.nodes.new('ShaderNodeTexImage')
        tex.image = normal_image
        tex.location = (-800, -400)
        nm = nt.nodes.new('ShaderNodeNormalMap')
        nm.location = (-500, -400)
        nm.inputs['Strength'].default_value = params["normal_strength"]
        nt.links.new(tex.outputs['Color'], nm.inputs['Color'])
        nt.links.new(nm.outputs['Normal'], bsdf.inputs['Normal'])
    # Explicit dielectric specular sanity (avoid artist-free sparkle)
    spec = bsdf.inputs.get('Specular IOR Level')
    if spec is not None and not spec.is_linked:
        spec.default_value = 0.5
    return mat


def stage_material_segmentation(profile):
    """Split the single baked Hunyuan material into semantic PBR classes
    (paint / glass / rubber / chrome) so each part shades correctly."""
    log("Segmenting materials into semantic PBR classes...")
    stats = {"attempted": False, "classes": [], "face_counts": {},
             "objects_segmented": 0, "skipped_reason": None}
    if profile != "vehicle":
        stats["skipped_reason"] = "non-vehicle profile"
        log(f"Segmentation skipped: {stats['skipped_reason']}", "WARN")
        return stats
    base_mat, _tex_node, base_image = _find_dominant_albedo()
    if base_image is None:
        stats["skipped_reason"] = "no albedo texture"
        log(f"Segmentation skipped: {stats['skipped_reason']}", "WARN")
        return stats
    import numpy as np
    arr = np.array(base_image.pixels[:], dtype=np.float32).reshape(
        base_image.size[1], base_image.size[0], 4)
    stats["attempted"] = True

    # Scene z-bounds for normalized height
    zmin, zmax = 1e9, -1e9
    for obj in get_scene_mesh_objects():
        for corner in obj.bound_box:
            zc = (obj.matrix_world @ Vector(corner)).z
            zmin = min(zmin, zc)
            zmax = max(zmax, zc)

    # Derive the detail normal map up front so the paint class can use it
    # (segmentation runs before the PBR finish pass).
    normal_image = None
    try:
        normal_image = _derive_normal_map(base_image, strength=0.5)
        if normal_image is not None:
            normal_image.pack()
            stats["normal_maps_created"] = 1
    except Exception as exc:
        log(f"normal map derivation skipped: {exc}", "WARN")

    class_counts = {"paint": 0, "glass": 0, "rubber": 0, "chrome": 0}
    made_mats = {}

    for obj in get_scene_mesh_objects():
        slots = [s.material for s in obj.material_slots]
        if len(slots) != 1 or slots[0] is None:
            continue
        rows = _sample_face_albedo(obj, arr)
        if not rows:
            continue
        classes = _classify_vehicle_faces(rows, (zmin, zmax))
        present = sorted(set(classes))
        if len(present) < 2:
            continue  # single-class object gains nothing from segmentation
        # Create one material per present class
        for cls in present:
            if cls not in made_mats:
                made_mats[cls] = _build_class_material(
                    slots[0], base_image, cls, normal_image)
        # Deterministic slot layout: first class replaces slot 0, rest are
        # appended. No pop()/reindex tricks - those scramble indices.
        index_of = {}
        for i, cls in enumerate(present):
            if i == 0:
                obj.data.materials[0] = made_mats[cls]
            else:
                obj.data.materials.append(made_mats[cls])
            index_of[cls] = i
        for poly, cls in zip(obj.data.polygons, classes):
            poly.material_index = index_of[cls]
        for cls in classes:
            class_counts[cls] += 1
        stats["objects_segmented"] += 1

    present = [c for c in ("paint", "glass", "rubber", "chrome") if class_counts[c] > 0]
    stats["classes"] = present
    stats["face_counts"] = {k: v for k, v in class_counts.items() if v > 0}
    log(f"Segmentation complete: classes={present} faces={stats['face_counts']}")
    if len(present) < 2:
        stats["skipped_reason"] = "classification found a single class"
    return stats


def stage_shading(angle_deg=35.0):
    """Smooth shading with hard edges above the angle threshold so hard-surface
    panel lines read crisply instead of mushy."""
    log(f"Applying smooth-by-angle shading ({angle_deg} deg)...")
    stats = {"objects": 0, "hard_edges": 0, "faces_smoothed": 0}
    threshold = math.radians(angle_deg)
    for obj in get_scene_mesh_objects():
        bm = bmesh.new()
        bm.from_mesh(obj.data)
        for f in bm.faces:
            f.smooth = True
            stats["faces_smoothed"] += 1
        for e in bm.edges:
            if len(e.link_faces) == 2:
                try:
                    if e.calc_face_angle() > threshold:
                        e.smooth = False
                        stats["hard_edges"] += 1
                except ValueError:
                    continue
        bm.to_mesh(obj.data)
        bm.free()
        stats["objects"] += 1
    log(f"Shading complete: {stats}")
    return stats


def stage_bake_ao_artifact(output_dir, img_size=1024, samples=48):
    """Bake an ambient-occlusion map using the non-overlapping lightmap UV and
    save it as ao.png next to the GLB. Unity does not auto-consume glTF
    occlusion textures, so we ship the PNG as an artist-ready artifact and do
    NOT claim it is wired into the material."""
    stats = {"attempted": True, "baked": False, "saved": None, "reason": None}
    try:
        objs = get_scene_mesh_objects()
        if not objs:
            stats["reason"] = "no mesh objects"
            return stats
        for obj in objs:
            lm = obj.data.uv_layers.get("lightmap") or (obj.data.uv_layers[1] if len(obj.data.uv_layers) > 1 else None)
            if lm is None:
                stats["reason"] = "no non-overlapping UV channel"
                return stats
        scene = bpy.context.scene
        scene.render.engine = 'CYCLES'
        scene.cycles.samples = samples
        scene.cycles.device = 'CPU'
        ao_img = bpy.data.images.new("Nexora_AO", width=img_size, height=img_size, alpha=False)
        for obj in objs:
            obj.select_set(True)
        bpy.context.view_layer.objects.active = objs[0]
        # Bake with the lightmap UV active for render/baking
        for obj in objs:
            lm = obj.data.uv_layers.get("lightmap") or obj.data.uv_layers[1]
            lm.active_render = True
        bpy.ops.object.bake(type='AO', use_clear=True, margin=8, pass_filter={'COLOR'})
        for obj in objs:
            lm = obj.data.uv_layers.get("lightmap") or obj.data.uv_layers[1]
            lm.active_render = False
        out_path = Path(output_dir) / "ao.png"
        ao_img.filepath_raw = str(out_path)
        ao_img.file_format = 'PNG'
        ao_img.save()
        ao_img.pack()
        stats["baked"] = True
        stats["saved"] = str(out_path)
        log(f"AO artifact baked: {out_path}")
    except Exception as exc:
        stats["reason"] = str(exc)
        log(f"AO bake skipped: {exc}", "WARN")
    return stats


def _purge_orphan_materials():
    """Remove materials with zero users (Hunyuan leftovers like 'Dots Stroke')."""
    removed = []
    for mat in list(bpy.data.materials):
        if mat.users == 0:
            removed.append(mat.name)
            bpy.data.materials.remove(mat)
    if removed:
        log(f"Purged orphan materials: {removed}")
    return {"removed": removed}

def stage_lightmap_uvs():
    """Stage: ensure UV0 exists and add a non-overlapping UV1 for lightmaps."""
    log("Ensuring lightmap UV channel (UV1)...")
    stats = {"objects": 0, "uv1_created": 0, "uv1_failed": 0}
    for obj in get_scene_mesh_objects():
        stats["objects"] += 1
        uvs = obj.data.uv_layers
        if not uvs:
            bpy.context.view_layer.objects.active = obj
            bpy.ops.object.mode_set(mode='EDIT')
            bpy.ops.mesh.select_all(action='SELECT')
            bpy.ops.uv.cube_project(cube_size=1.0)
            bpy.ops.object.mode_set(mode='OBJECT')
            uvs = obj.data.uv_layers
        if not uvs:
            stats["uv1_failed"] += 1
            continue
        if len(uvs) == 1:
            uvs[0].name = "UVMap"
        if "lightmap" in uvs:
            continue
        try:
            uvs.new(name="lightmap")
            lm = uvs["lightmap"]
            uvs.active = lm
            lm.active_render = False
            bpy.context.view_layer.objects.active = obj
            bpy.ops.object.mode_set(mode='EDIT')
            bpy.ops.mesh.select_all(action='SELECT')
            bpy.ops.uv.smart_project(angle_limit=66.0, island_margin=0.005)
            bpy.ops.object.mode_set(mode='OBJECT')
            stats["uv1_created"] += 1
        except Exception as exc:
            log(f"lightmap UV failed on {obj.name}: {exc}", "WARN")
            stats["uv1_failed"] += 1
    log(f"Lightmap UV complete: {stats}")
    return stats


def stage_optimize(quality, target_tris):
    """Stage 6: Target decimation."""
    log(f"Optimizing for {quality} (target: {target_tris} tris)...")
    if target_tris is None:
        return get_scene_stats()
    
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        current_tris = get_object_triangle_count(obj)
        if current_tris <= target_tris:
            continue
        
        ratio = max(0.1, min(1.0, target_tris / current_tris))
        mod = obj.modifiers.new(name="NexoraDecimate", type='DECIMATE')
        mod.ratio = ratio
        mod.use_collapse_triangulate = True
        bpy.ops.object.modifier_apply(modifier=mod.name)
    
    stats = get_scene_stats()
    log(f"Optimization complete: {stats}")
    return stats

def stage_lod_generation(quality, output_dir):
    """Stage 7: Multi-Level of Detail (LOD0, LOD1, LOD2) generation."""
    log("Generating LODs (LOD0, LOD1, LOD2)...")
    lod_targets = {
        "master": [1.0, 0.5, 0.25],
        "mobile_high": [1.0, 0.5, 0.25],
        "mobile_balanced": [1.0, 0.4, 0.15],
        "mobile_low": [1.0, 0.3, 0.1],
    }
    ratios = lod_targets.get(quality, [1.0, 0.5, 0.25])
    lod_paths = {}
    original_objects = [obj for obj in get_scene_mesh_objects()]
    
    for lod_idx, ratio in enumerate(ratios):
        if lod_idx == 0:
            lod_paths["lod0"] = "master"
            continue
            
        lod_objects = []
        for obj in original_objects:
            dup = obj.copy()
            dup.data = obj.data.copy()
            bpy.context.collection.objects.link(dup)
            lod_objects.append(dup)
        
        for obj in lod_objects:
            bpy.context.view_layer.objects.active = obj
            mod = obj.modifiers.new(name=f"LOD{lod_idx}_Decimate", type='DECIMATE')
            mod.ratio = ratio
            mod.use_collapse_triangulate = True
            bpy.ops.object.modifier_apply(modifier=mod.name)
        
        lod_filename = f"vehicle_lod{lod_idx}.glb"
        lod_full_path = str(output_dir / lod_filename)
        export_objects(lod_objects, lod_full_path)
        lod_paths[f"lod{lod_idx}"] = lod_filename
        
        for obj in lod_objects:
            mesh_data = obj.data
            bpy.data.objects.remove(obj, do_unlink=True)
            if mesh_data and mesh_data.users == 0:
                bpy.data.meshes.remove(mesh_data)
    
    log(f"LOD generation complete: {lod_paths}")
    return lod_paths

def stage_generate_collision(output_dir):
    """Stage 8: Automated Collision Geometry Generation (Convex Hull)."""
    log("Generating simplified physics collision geometry...")
    objects = get_scene_mesh_objects()
    if not objects:
        return {"generated": False, "triangles": 0}
    
    bm = bmesh.new()
    for obj in objects:
        bm_temp = bmesh.new()
        bm_temp.from_mesh(obj.data)
        for v in bm_temp.verts:
            v.co = obj.matrix_world @ v.co
            bm.verts.new(v.co)
        bm_temp.free()
    
    bmesh.ops.convex_hull(bm, input=bm.verts)
    col_mesh = bpy.data.meshes.new("Asset_Collider")
    bm.to_mesh(col_mesh)
    bm.free()
    
    col_obj = bpy.data.objects.new("Asset_Collider", col_mesh)
    bpy.context.collection.objects.link(col_obj)
    
    bpy.context.view_layer.objects.active = col_obj
    col_tris = get_object_triangle_count(col_obj)
    if col_tris > 64:
        mod = col_obj.modifiers.new(name="ColliderDecimate", type='DECIMATE')
        mod.ratio = max(0.2, 64.0 / col_tris)
        mod.use_collapse_triangulate = True
        bpy.ops.object.modifier_apply(modifier=mod.name)
    
    final_col_tris = get_object_triangle_count(col_obj)
    col_path = output_dir / "clean_collider.glb"
    export_objects([col_obj], str(col_path))
    
    bpy.data.objects.remove(col_obj, do_unlink=True)
    bpy.data.meshes.remove(col_mesh)
    
    log(f"Collision geometry generated: {col_path} ({final_col_tris} triangles)")
    return {
        "generated": True,
        "filename": "clean_collider.glb",
        "triangles": final_col_tris,
        "type": "convex_hull",
    }

def stage_vehicle_analysis():
    """Stage 9: Vehicle Structure & Wheel Candidate Detection."""
    log("Running vehicle structural analysis...")
    objects = get_scene_mesh_objects()
    wheel_candidates = []
    
    for obj in objects:
        dims = obj.dimensions
        xy_ratio = min(dims.x, dims.y) / max(dims.x, dims.y) if max(dims.x, dims.y) > 0 else 0
        if xy_ratio > 0.7:
            wheel_candidates.append(obj)
    
    wheel_count = len(wheel_candidates)
    wheel_separation_possible = wheel_count >= 4
    body = max(objects, key=lambda o: o.dimensions.x * o.dimensions.y * o.dimensions.z) if objects else None
    
    orientation_confidence = 0.8 if (body and body.dimensions.y >= body.dimensions.x) else 0.5
    
    analysis = {
        "body_detected": body is not None,
        "body_object": body.name if body else None,
        "wheel_candidates": wheel_count,
        "wheel_separation_possible": wheel_separation_possible,
        "confidence": orientation_confidence * (0.9 if wheel_separation_possible else 0.5),
        "needs_review": not wheel_separation_possible,
        "wheel_objects": [w.name for w in wheel_candidates],
    }
    return analysis


def stage_character_analysis():
    """Stage 9b: Character Structure & Skeleton Detection."""
    log("Running character structural analysis...")
    objects = get_scene_mesh_objects()
    
    # Look for armature
    armatures = [obj for obj in bpy.context.scene.objects if obj.type == 'ARMATURE']
    has_armature = len(armatures) > 0
    
    # Check for skinning (vertex groups)
    skinned_meshes = 0
    for obj in objects:
        if obj.vertex_groups:
            skinned_meshes += 1
    
    # Find main body (largest mesh)
    body = max(objects, key=lambda o: o.dimensions.x * o.dimensions.y * o.dimensions.z) if objects else None
    
    # Check approximate humanoid proportions
    humanoid_confidence = 0.0
    if body:
        dims = body.dimensions
        height = dims.z
        width = max(dims.x, dims.y)
        if height > 0 and width > 0:
            ratio = height / width
            # Humanoid roughly 2:1 to 3:1 height:width
            if 1.5 <= ratio <= 4.0:
                humanoid_confidence = 0.7
    
    # Check for animation data
    has_animations = False
    for action in bpy.data.actions:
        if action.users > 0:
            has_animations = True
            break
    
    rig_status = "UNRIGGED"
    if has_armature and skinned_meshes > 0:
        rig_status = "RIGGED"
    elif has_armature:
        rig_status = "ARMATURE_ONLY"
    elif skinned_meshes > 0:
        rig_status = "VERTEX_GROUPS_ONLY"
    
    animated_status = "NOT_ANIMATED"
    if has_animations and rig_status == "RIGGED":
        animated_status = "ANIMATED"
    elif has_animations:
        animated_status = "ANIMATION_DATA_ONLY"
    
    analysis = {
        "body_detected": body is not None,
        "body_object": body.name if body else None,
        "has_armature": has_armature,
        "armature_count": len(armatures),
        "skinned_meshes": skinned_meshes,
        "humanoid_confidence": humanoid_confidence,
        "rig_status": rig_status,
        "animation_status": animated_status,
        "needs_review": rig_status != "RIGGED",
        "character_ready": rig_status == "RIGGED",
        "rigged_ready": rig_status == "RIGGED" and humanoid_confidence > 0.5,
        "animated_ready": animated_status == "ANIMATED",
    }
    return analysis


def stage_prop_analysis():
    """Stage 9c: Prop Structure Analysis."""
    log("Running prop structural analysis...")
    objects = get_scene_mesh_objects()
    
    # Find main object
    main_obj = max(objects, key=lambda o: o.dimensions.x * o.dimensions.y * o.dimensions.z) if objects else None
    
    # Check for multiple parts
    part_count = len(objects)
    
    analysis = {
        "main_object": main_obj.name if main_obj else None,
        "part_count": part_count,
        "single_part": part_count == 1,
        "needs_review": part_count > 10,  # Many parts might need organization
    }
    return analysis


def stage_weapon_analysis():
    """Stage 9d: Weapon Structure Analysis."""
    log("Running weapon structural analysis...")
    objects = get_scene_mesh_objects()
    
    # Find longest object (likely barrel/main body)
    main_obj = max(objects, key=lambda o: max(o.dimensions.x, o.dimensions.y, o.dimensions.z)) if objects else None
    
    # Check for grip/trigger area (smaller parts near one end)
    grip_candidates = []
    if main_obj:
        main_dims = main_obj.dimensions
        main_length = max(main_dims.x, main_dims.y, main_dims.z)
        for obj in objects:
            if obj != main_obj:
                obj_dims = obj.dimensions
                obj_max = max(obj_dims.x, obj_dims.y, obj_dims.z)
                if obj_max < main_length * 0.3:
                    grip_candidates.append(obj)
    
    analysis = {
        "main_object": main_obj.name if main_obj else None,
        "grip_candidates": len(grip_candidates),
        "part_count": len(objects),
        "needs_review": len(grip_candidates) == 0,
    }
    return analysis


def stage_building_analysis():
    """Stage 9e: Building/Modular Structure Analysis."""
    log("Running building structural analysis...")
    objects = get_scene_mesh_objects()
    
    # Check for modularity (similar sized parts)
    dims_list = [(obj.dimensions.x, obj.dimensions.y, obj.dimensions.z) for obj in objects]
    volumes = [x * y * z for x, y, z in dims_list]
    
    modular_score = 0.0
    if volumes:
        avg_vol = sum(volumes) / len(volumes)
        similar_count = sum(1 for v in volumes if abs(v - avg_vol) / avg_vol < 0.5)
        modular_score = similar_count / len(volumes)
    
    analysis = {
        "part_count": len(objects),
        "modular_score": modular_score,
        "likely_modular": modular_score > 0.5,
        "needs_review": modular_score < 0.3 and len(objects) > 5,
    }
    return analysis


def stage_vegetation_analysis():
    """Stage 9f: Vegetation Analysis."""
    log("Running vegetation analysis...")
    objects = get_scene_mesh_objects()
    
    # Check for billboard-ready (flat-ish) objects
    flat_objects = 0
    for obj in objects:
        dims = obj.dimensions
        sorted_dims = sorted([dims.x, dims.y, dims.z])
        if sorted_dims[0] < sorted_dims[1] * 0.1:  # Very thin in one dimension
            flat_objects += 1
    
    analysis = {
        "part_count": len(objects),
        "flat_objects": flat_objects,
        "billboard_candidates": flat_objects,
        "needs_review": flat_objects == 0 and len(objects) == 1,  # Single non-flat might be tree trunk
    }
    return analysis


def stage_environment_analysis():
    """Stage 9g: Environment/Level Geometry Analysis."""
    log("Running environment analysis...")
    objects = get_scene_mesh_objects()
    
    # Check for ground plane
    ground_candidates = []
    for obj in objects:
        dims = obj.dimensions
        if dims.z < 0.5 and (dims.x > 5 or dims.y > 5):  # Flat and large
            ground_candidates.append(obj)
    
    analysis = {
        "part_count": len(objects),
        "ground_planes": len(ground_candidates),
        "likely_modular": len(objects) > 10,
        "needs_review": len(ground_candidates) == 0,
    }
    return analysis


def stage_furniture_analysis():
    """Stage 9h: Furniture Analysis."""
    log("Running furniture analysis...")
    objects = get_scene_mesh_objects()
    
    # Check for upright orientation (height > width/depth)
    upright_count = 0
    for obj in objects:
        dims = obj.dimensions
        if dims.z > dims.x * 1.2 and dims.z > dims.y * 1.2:
            upright_count += 1
    
    analysis = {
        "part_count": len(objects),
        "upright_objects": upright_count,
        "needs_review": upright_count == 0,
    }
    return analysis


def stage_equipment_analysis():
    """Stage 9i: Equipment Analysis."""
    log("Running equipment analysis...")
    objects = get_scene_mesh_objects()
    
    main_obj = max(objects, key=lambda o: o.dimensions.x * o.dimensions.y * o.dimensions.z) if objects else None
    
    analysis = {
        "main_object": main_obj.name if main_obj else None,
        "part_count": len(objects),
        "compact": all(max(o.dimensions.x, o.dimensions.y, o.dimensions.z) < 2.0 for o in objects),
        "needs_review": False,
    }
    return analysis


def run_category_analysis(profile, output_dir):
    """Run category-specific analysis based on profile."""
    config = CATEGORY_PROCESSING_CONFIG.get(profile, CATEGORY_PROCESSING_CONFIG["generic"])
    
    if profile == "vehicle":
        return stage_vehicle_analysis()
    elif profile == "character":
        return stage_character_analysis()
    elif profile == "prop":
        return stage_prop_analysis()
    elif profile == "weapon":
        return stage_weapon_analysis()
    elif profile == "building":
        return stage_building_analysis()
    elif profile == "vegetation":
        return stage_vegetation_analysis()
    elif profile == "environment":
        return stage_environment_analysis()
    elif profile == "furniture":
        return stage_furniture_analysis()
    elif profile == "equipment":
        return stage_equipment_analysis()
    else:
        return {"profile": profile, "note": "Generic processing applied"}

def stage_validate(output_dir):
    """Stage 10: Final master export & validation."""
    log("Exporting and validating clean master GLB...")
    master_path = output_dir / "clean_master.glb"
    export_objects(get_scene_mesh_objects(), str(master_path))
    
    if not master_path.exists() or master_path.stat().st_size == 0:
        raise RuntimeError("Master export failed or is 0 bytes")
    
    return {
        "master_exists": True,
        "master_size": master_path.stat().st_size,
    }

def calculate_quality_score(report, final_stats):
    """Calculate an authentic 0-100 AAA Game Readiness Score."""
    score = 0
    breakdown = {}
    
    # 1. Topology & Geometry Cleanliness (15 pts)
    degen = report["stages"].get("analyze", {}).get("analysis", {}).get("degenerate_faces", 0)
    non_man = report["stages"].get("repair", {}).get("stats", {}).get(
        "non_manifold_after",
        report["stages"].get("analyze", {}).get("analysis", {}).get("non_manifold_edges", 0))
    topo_score = 15
    if degen > 0: topo_score -= min(6, degen)
    if non_man > 50: topo_score -= 6
    elif non_man > 10: topo_score -= 3
    score += max(6, topo_score)
    breakdown["topology"] = max(6, topo_score)

    # 2. Normal Consistency & Coordinate Normalization (10 pts)
    norm_score = 10 if report["stages"].get("repair", {}).get("status") == "completed" else 0
    score += norm_score
    breakdown["normals_and_transforms"] = norm_score

    # 3. Smooth-by-angle shading (10 pts) - hard-surface edge readability
    shade = report["stages"].get("shading", {}).get("stats", {})
    shade_score = 0
    if shade.get("objects", 0) > 0:
        shade_score = 4
        if shade.get("hard_edges", 0) > 50:
            shade_score = 10  # real panel lines detected
        elif shade.get("hard_edges", 0) > 10:
            shade_score = 7
    score += shade_score
    breakdown["shading"] = shade_score

    # 4. LOD0/LOD1/LOD2 Hierarchy (15 pts)
    lod_score = 15 if "lod_generation" in report["stages"] else 0
    score += lod_score
    breakdown["lod_hierarchy"] = lod_score

    # 5. Simplified Collision Geometry (10 pts)
    col_score = 10 if report["stages"].get("collision", {}).get("generated") else 0
    score += col_score
    breakdown["collision_mesh"] = col_score

    # 6. PBR Material Readiness (30 pts) - graded on real semantic content
    seg = report["stages"].get("material_segmentation", {}).get("stats", {})
    pbr = report["stages"].get("pbr_materials", {}).get("stats", {})
    mat_score = 0
    if final_stats.get("material_count", 0) > 0:
        mat_score += 3
    classes = seg.get("classes", [])
    if len(classes) >= 4:
        mat_score += 12  # paint/glass/rubber/chrome all present
    elif len(classes) == 3:
        mat_score += 9
    elif len(classes) == 2:
        mat_score += 6
    if "paint" in classes and final_stats.get("material_count", 0) > 1:
        mat_score += 4  # clearcoat car paint assigned
    if pbr.get("normal_maps_created", 0) > 0:
        mat_score += 5
    if pbr.get("roughness_maps_created", 0) > 0 and not seg.get("classes"):
        mat_score += 4  # legacy single-material path only
    if pbr.get("sanity_verified") or pbr.get("metallic_clamped", 0) > 0:
        mat_score += 2
    score += min(30, mat_score)
    breakdown["materials"] = min(30, mat_score)

    # 7. Lightmap UV1 (5 pts)
    lm = report["stages"].get("lightmap_uvs", {}).get("stats", {})
    lm_score = 5 if lm.get("uv1_created", 0) > 0 else 0
    score += lm_score
    breakdown["lightmap_uvs"] = lm_score

    # 8. AO artifact (5 pts) - honest: artifact only, not shader-wired
    ao = report["stages"].get("ao_bake", {}).get("stats", {})
    ao_score = 5 if ao.get("baked") else 0
    score += ao_score
    breakdown["ao_artifact"] = ao_score
    
    # 6. Category-Specific Readiness (bonus/penalty, not part of base 100)
    cat_bonus = 0
    cat_analysis = report["stages"].get("category_analysis", {}).get("analysis", {})
    if cat_analysis.get("needs_review") is False:
        cat_bonus = 5  # Small bonus for category-specific readiness
    elif cat_analysis.get("needs_review") is True:
        cat_bonus = -5  # Penalty for category-specific issues
    breakdown["category_readiness"] = cat_bonus
    
    # Clamp to 100
    final_score = min(100, max(0, score + cat_bonus))
    rating = "AAA_GAME_READY" if final_score >= 90 else "PRODUCTION_READY" if final_score >= 75 else "NEEDS_REVIEW"
    if final_score >= 90 and len(report.get("stages", {}).get("material_segmentation", {}).get("stats", {}).get("classes", [])) < 3:
        rating = "PRODUCTION_READY"  # AAA requires semantic material classes
    return final_score, rating, breakdown

# ============================================================================
# Main Processing Pipeline Entry Point
# ============================================================================

def main():
    parser = argparse.ArgumentParser(description="Nexora 3D Mesh Processor")
    parser.add_argument("--input", required=True, help="Input model file")
    parser.add_argument("--output", required=True, help="Output directory")
    parser.add_argument("--profile", choices=["generic", "vehicle", "character", "environment", "prop", "weapon", "building", "vegetation", "furniture", "equipment"], default="generic")
    parser.add_argument("--quality", choices=["master", "mobile_high", "mobile_balanced", "mobile_low"], default="mobile_high")
    parser.add_argument("--report", required=True, help="Output report JSON path")
    parser.add_argument("--log", help="Optional log file path")

    argv_clean = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else sys.argv[1:]
    args = parser.parse_args(argv_clean)

    if args.log:
        sys.stdout = open(args.log, 'w')

    log("=" * 70)
    log("NEXORA AAA 3D MESH PROCESSOR")
    log(f"Input:    {args.input}")
    log(f"Output:   {args.output}")
    log(f"Profile:  {args.profile}")
    log(f"Quality:  {args.quality}")
    log("=" * 70)
    
    input_path = Path(args.input)
    output_dir = Path(args.output)
    report_path = Path(args.report)
    
    if not input_path.exists():
        raise FileNotFoundError(f"Input file not found: {input_path}")
    
    output_dir.mkdir(parents=True, exist_ok=True)
    
    report = {
        "input": str(input_path),
        "output_dir": str(output_dir),
        "profile": args.profile,
        "quality": args.quality,
        "stages": {},
        "errors": [],
        "warnings": [],
    }
    
    try:
        bpy.ops.object.select_all(action='SELECT')
        bpy.ops.object.delete()
        
        # 1. Import
        stats_before = stage_import(args.input, output_dir)
        report["stages"]["import"] = {"status": "completed", "stats": stats_before}
        
        # 2. Analyze
        analysis = stage_analyze(stats_before)
        report["stages"]["analyze"] = {"status": "completed", "analysis": analysis}
        report["warnings"].extend(analysis.get("warnings", []))
        
        # 3. Clean
        stats_clean = stage_clean()
        report["stages"]["clean"] = {"status": "completed", "stats": stats_clean}
        
        # 4. Scale & Coordinate Normalization
        norm_result = stage_normalize_scale_and_origin(args.profile)
        report["stages"]["normalization"] = {"status": "completed", "details": norm_result}
        
        # 5. Repair
        stats_repair = stage_repair()
        report["stages"]["repair"] = {"status": "completed", "stats": stats_repair}

        # 5b. Semantic material segmentation (paint/glass/rubber/chrome)
        seg_stats = stage_material_segmentation(args.profile)
        report["stages"]["material_segmentation"] = {"status": "completed", "stats": seg_stats}

        # 5c. PBR material finish (normal/roughness + dielectric sanity)
        pbr_stats = stage_pbr_materials(segmentation=seg_stats)
        report["stages"]["pbr_materials"] = {"status": "completed", "stats": pbr_stats}

        # 5d. Smooth-by-angle shading for crisp hard-surface edges
        shade_stats = stage_shading()
        report["stages"]["shading"] = {"status": "completed", "stats": shade_stats}

        # 5e. Lightmap UV channel (UV1) for baked GI in-engine
        lm_stats = stage_lightmap_uvs()
        report["stages"]["lightmap_uvs"] = {"status": "completed", "stats": lm_stats}

        # 5f. AO bake artifact (saved as ao.png using the lightmap UV)
        ao_stats = stage_bake_ao_artifact(output_dir)
        report["stages"]["ao_bake"] = {"status": "completed", "stats": ao_stats}

        # 5g. Purge orphan materials left by generation
        purge_stats = _purge_orphan_materials()
        report["stages"]["purge_orphans"] = {"status": "completed", "stats": purge_stats}
        
        # 6. Optimize
        target_tris = QUALITY_TRIANGLE_TARGETS.get(args.quality, 50000)
        stats_optimize = stage_optimize(args.quality, target_tris)
        report["stages"]["optimize"] = {"status": "completed", "target_triangles": target_tris, "stats": stats_optimize}
        
        # 7. LOD0/1/2 Generation
        lod_paths = stage_lod_generation(args.quality, output_dir)
        report["stages"]["lod_generation"] = {"status": "completed", "lod_paths": lod_paths}
        
        # 8. Collision Generation
        col_result = stage_generate_collision(output_dir)
        report["stages"]["collision"] = col_result
        
        # 9. Category-Specific Analysis
        cat_analysis = run_category_analysis(args.profile, output_dir)
        report["stages"]["category_analysis"] = {"status": "completed", "analysis": cat_analysis}
        
        # 10. Master Export & Validation
        validation = stage_validate(output_dir)
        report["stages"]["validate"] = {"status": "completed", "validation": validation}
        
        # Final Summary & Quality Score
        final_stats = get_scene_stats()
        score, rating, breakdown = calculate_quality_score(report, final_stats)
        
        report["quality_score"] = score
        report["quality_rating"] = rating
        report["score_breakdown"] = breakdown
        report["final_status"] = "READY_FOR_REVIEW"
        
        report["triangle_counts"] = {
            "pre_clean": stats_before.get("triangle_count", 0),
            "post_clean": final_stats.get("triangle_count", 0),
            "lod0": "master",
            "lod1": lod_paths.get("lod1"),
            "lod2": lod_paths.get("lod2"),
        }
        report["material_status"] = "ready" if final_stats.get("material_count", 0) > 0 else "standard_pbr"
        
        log(f"Quality Score: {score}/100 ({rating})")
        
    except Exception as e:
        report["errors"].append(str(e))
        report["final_status"] = "FAILED"
        log(f"FATAL ERROR: {e}", "ERROR")
        traceback.print_exc()
    
    write_json(report_path, report)
    log(f"Report successfully written to {report_path}")

if __name__ == "__main__":
    main()