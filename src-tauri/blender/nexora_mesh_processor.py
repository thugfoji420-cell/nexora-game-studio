#!/usr/bin/env python3
"""
Nexora 3D Mesh Processor - Blender Headless Script

This script runs inside Blender headless mode to process 3D assets through
the Nexora pipeline stages: importing, analyzing, cleaning, repairing,
optimizing, LOD generation, and validation.

Usage:
    blender.exe --background --python nexora_mesh_processor.py -- \
        --input <source.glb> \
        --output <job_output_dir> \
        --profile <generic|vehicle> \
        --quality <master|mobile_high|mobile_balanced|mobile_low> \
        --report <report.json>
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

MERGE_DISTANCE = 0.0001  # Conservative merge-by-distance threshold
DEGENERATE_AREA_THRESHOLD = 1e-8
MIN_COMPONENT_VERTICES = 10  # Remove components with fewer vertices

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
    # Triangulate if needed
    import bmesh
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

# ============================================================================
# Processing Stages
# ============================================================================

def stage_import(input_path, output_dir):
    """Stage 1: Import the source GLB/GLTF/FBX/OBJ file."""
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
    
    # Save working copy
    working_path = output_dir / "working.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(working_path))
    
    stats = get_scene_stats()
    log(f"Imported: {stats}")
    return stats

def stage_analyze(stats_before):
    """Stage 2: Analyze the imported mesh."""
    log("Analyzing mesh...")
    
    stats_after = get_scene_stats()
    
    # Non-manifold detection
    bpy.ops.object.mode_set(mode='EDIT')
    bpy.ops.mesh.select_all(action='DESELECT')
    bpy.ops.mesh.select_non_manifold()
    bpy.ops.object.mode_set(mode='OBJECT')
    
    non_manifold_count = 0
    for obj in get_scene_mesh_objects():
        mesh = obj.data
        for edge in mesh.edges:
            if len(edge.link_faces) != 2:
                non_manifold_count += 1
    
    # Degenerate faces
    degenerate_count = 0
    for obj in get_scene_mesh_objects():
        mesh = obj.data
        for poly in mesh.polygons:
            if poly.area < DEGENERATE_AREA_THRESHOLD:
                degenerate_count += 1
    
    # Loose geometry
    loose_verts = 0
    loose_edges = 0
    for obj in get_scene_mesh_objects():
        mesh = obj.data
        for vert in mesh.vertices:
            if len(vert.link_edges) == 0:
                loose_verts += 1
        for edge in mesh.edges:
            if len(edge.link_faces) == 0:
                loose_edges += 1
    
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
    if loose_verts > 0:
        analysis["warnings"].append(f"Found {loose_verts} loose vertices")
    
    log(f"Analysis complete: {analysis}")
    return analysis

def stage_clean():
    """Stage 3: Conservative cleanup - remove loose geometry, degenerate faces."""
    log("Cleaning mesh...")
    
    bpy.ops.object.mode_set(mode='OBJECT')
    
    # Remove loose vertices/edges
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='DESELECT')
        bpy.ops.mesh.select_loose()
        bpy.ops.mesh.delete(type='VERT')
        bpy.ops.object.mode_set(mode='OBJECT')
    
    # Remove degenerate faces (zero area)
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='DESELECT')
        # Select faces with near-zero area
        import bmesh
        bm = bmesh.from_edit_mesh(obj.data)
        for face in bm.faces:
            if face.calc_area() < DEGENERATE_AREA_THRESHOLD:
                face.select = True
        if any(f.select for f in bm.faces):
            bmesh.ops.delete(bm, geom=[f for f in bm.faces if f.select], context='FACES')
        bmesh.update_edit_mesh(obj.data)
        bpy.ops.object.mode_set(mode='OBJECT')
    
    stats = get_scene_stats()
    log(f"Clean complete: {stats}")
    return stats

def stage_repair():
    """Stage 4: Repair - normals, transform normalization."""
    log("Repairing mesh...")
    
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        
        # Apply transforms
        bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
        
        # Recalculate normals
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.mesh.normals_make_consistent(inside=False)
        bpy.ops.object.mode_set(mode='OBJECT')
        
        # Remove doubles (conservative merge by distance)
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.mesh.remove_doubles(threshold=MERGE_DISTANCE)
        bpy.ops.object.mode_set(mode='OBJECT')
    
    stats = get_scene_stats()
    log(f"Repair complete: {stats}")
    return stats

def stage_optimize(quality, target_tris):
    """Stage 5: Optimization - conservative decimation."""
    log(f"Optimizing for {quality} (target: {target_tris} tris)...")
    
    if target_tris is None:
        log("Master quality - skipping decimation")
        return get_scene_stats()
    
    for obj in get_scene_mesh_objects():
        bpy.context.view_layer.objects.active = obj
        
        current_tris = get_object_triangle_count(obj)
        if current_tris <= target_tris:
            log(f"Object {obj.name} already at target ({current_tris} <= {target_tris})")
            continue
        
        ratio = target_tris / current_tris
        ratio = max(0.1, min(1.0, ratio))  # Clamp between 10% and 100%
        
        log(f"Decimating {obj.name}: {current_tris} -> ~{int(current_tris * ratio)} tris (ratio: {ratio:.2f})")
        
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.mode_set(mode='OBJECT')
        
        # Add decimate modifier
        mod = obj.modifiers.new(name="NexoraDecimate", type='DECIMATE')
        mod.ratio = ratio
        mod.use_collapse_triangulate = True
        
        # Apply modifier
        bpy.ops.object.modifier_apply(modifier=mod.name)
    
    stats = get_scene_stats()
    log(f"Optimization complete: {stats}")
    return stats

def stage_lod_generation(quality):
    """Stage 6: LOD Generation."""
    log("Generating LODs...")
    
    lod_targets = {
        "master": [None, 0.5, 0.25],
        "mobile_high": [1.0, 0.5, 0.25],
        "mobile_balanced": [1.0, 0.4, 0.15],
        "mobile_low": [1.0, 0.3, 0.1],
    }
    
    ratios = lod_targets.get(quality, [1.0, 0.5, 0.25])
    
    lod_paths = {}
    original_objects = [obj for obj in get_scene_mesh_objects()]
    
    for lod_idx, ratio in enumerate(ratios):
        if lod_idx == 0 and ratio == 1.0:
            # LOD0 is the original
            lod_paths[f"lod{lod_idx}"] = "master"
            continue
            
        log(f"Generating LOD{lod_idx} with ratio {ratio}")
        
        # Duplicate all objects
        lod_objects = []
        for obj in original_objects:
            dup = obj.copy()
            dup.data = obj.data.copy()
            bpy.context.collection.objects.link(dup)
            lod_objects.append(dup)
        
        # Decimate
        for obj in lod_objects:
            bpy.context.view_layer.objects.active = obj
            mod = obj.modifiers.new(name=f"LOD{lod_idx}_Decimate", type='DECIMATE')
            mod.ratio = ratio
            mod.use_collapse_triangulate = True
            bpy.ops.object.modifier_apply(modifier=mod.name)
        
        # Export LOD
        lod_path = f"vehicle_lod{lod_idx}.glb"
        export_objects(lod_objects, lod_path)
        lod_paths[f"lod{lod_idx}"] = lod_path
        
        # Clean up duplicates
        for obj in lod_objects:
            bpy.data.objects.remove(obj, do_unlink=True)
            if obj.data.users == 0:
                bpy.data.meshes.remove(obj.data)
    
    log(f"LOD generation complete: {lod_paths}")
    return lod_paths

def export_objects(objects, output_path):
    """Export selected objects as GLB."""
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

def stage_vehicle_analysis():
    """Stage 7: Vehicle analysis - detect wheels, body, orientation."""
    log("Running vehicle analysis...")
    
    objects = get_scene_mesh_objects()
    
    # Heuristic: find 4 roughly cylindrical objects of similar size (wheels)
    wheel_candidates = []
    for obj in objects:
        dims = obj.dimensions
        # Check if roughly cylindrical (two dimensions similar, one different)
        xy_ratio = min(dims.x, dims.y) / max(dims.x, dims.y) if max(dims.x, dims.y) > 0 else 0
        if xy_ratio > 0.7:  # Roughly circular in XY
            wheel_candidates.append(obj)
    
    wheel_count = len(wheel_candidates)
    wheel_separation_possible = wheel_count >= 4
    
    # Body detection - largest object by volume
    body = max(objects, key=lambda o: o.dimensions.x * o.dimensions.y * o.dimensions.z) if objects else None
    
    # Orientation - assume +Y is forward, +Z is up
    # Check if body is aligned
    orientation_confidence = 0.5
    if body:
        # Check if body's longest axis aligns with Y (forward)
        dims = body.dimensions
        if dims.y >= dims.x and dims.y >= dims.z:
            orientation_confidence = 0.8
    
    analysis = {
        "body_detected": body is not None,
        "body_object": body.name if body else None,
        "wheel_candidates": wheel_count,
        "wheel_separation_possible": wheel_separation_possible,
        "confidence": orientation_confidence * (0.8 if wheel_separation_possible else 0.4),
        "needs_review": not wheel_separation_possible,
        "wheel_objects": [w.name for w in wheel_candidates],
    }
    
    log(f"Vehicle analysis: {analysis}")
    return analysis

def stage_validate(output_dir):
    """Stage 8: Final validation - export and verify."""
    log("Validating output...")
    
    # Export master
    master_path = output_dir / "clean_master.glb"
    export_objects(get_scene_mesh_objects(), str(master_path))
    
    # Verify with model3d_validation (re-import and check)
    # For now, just verify file exists and has content
    if master_path.exists() and master_path.stat().st_size > 0:
        log(f"Master export verified: {master_path} ({master_path.stat().st_size} bytes)")
    else:
        raise RuntimeError("Master export failed or empty")
    
    # Run structural validation by re-importing
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete()
    
    bpy.ops.import_scene.gltf(filepath=str(master_path))
    imported_stats = get_scene_stats()
    
    validation = {
        "master_exists": True,
        "master_size": master_path.stat().st_size,
        "imported_vertices": imported_stats["vertex_count"],
        "imported_triangles": imported_stats["triangle_count"],
        "imported_objects": imported_stats["object_count"],
    }
    
    log(f"Validation: {validation}")
    return validation

# ============================================================================
# Main Processing Pipeline
# ============================================================================

def main():
    parser = argparse.ArgumentParser(description="Nexora 3D Mesh Processor")
    parser.add_argument("--input", required=True, help="Input model file (GLB/GLTF/FBX/OBJ/PLY)")
    parser.add_argument("--output", required=True, help="Output directory")
    parser.add_argument("--profile", choices=["generic", "vehicle"], default="vehicle")
    parser.add_argument("--quality", choices=["master", "mobile_high", "mobile_balanced", "mobile_low"], default="mobile_balanced")
    parser.add_argument("--report", required=True, help="Output report JSON path")
    parser.add_argument("--log", help="Optional log file path")
    
    args = parser.parse_args()
    
    if args.log:
        sys.stdout = open(args.log, 'w')
    
    log("=" * 60)
    log("Nexora 3D Mesh Processor Starting")
    log(f"Input: {args.input}")
    log(f"Output: {args.output}")
    log(f"Profile: {args.profile}")
    log(f"Quality: {args.quality}")
    log("=" * 60)
    
    input_path = Path(args.input)
    output_dir = Path(args.output)
    report_path = Path(args.report)
    
    if not input_path.exists():
        raise FileNotFoundError(f"Input file not found: {input_path}")
    
    output_dir.mkdir(parents=True, exist_ok=True)
    
    # Initialize report
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
        # Clear scene
        bpy.ops.object.select_all(action='SELECT')
        bpy.ops.object.delete()
        
        # Stage 1: Import
        stats_before = stage_import(args.input, output_dir)
        report["stages"]["import"] = {"status": "completed", "stats": stats_before}
        
        # Stage 2: Analyze
        analysis = stage_analyze(stats_before)
        report["stages"]["analyze"] = {"status": "completed", "analysis": analysis}
        report["warnings"].extend(analysis.get("warnings", []))
        
        # Stage 3: Clean
        stats_clean = stage_clean()
        report["stages"]["clean"] = {"status": "completed", "stats": stats_clean}
        
        # Stage 4: Repair
        stats_repair = stage_repair()
        report["stages"]["repair"] = {"status": "completed", "stats": stats_repair}
        
        # Stage 5: Optimize
        target_tris = QUALITY_TRIANGLE_TARGETS.get(args.quality)
        stats_optimize = stage_optimize(args.quality, target_tris)
        report["stages"]["optimize"] = {"status": "completed", "target_triangles": target_tris, "stats": stats_optimize}
        
        # Stage 6: LOD Generation
        lod_paths = stage_lod_generation(args.quality)
        report["stages"]["lod_generation"] = {"status": "completed", "lod_paths": lod_paths}
        
        # Stage 7: Vehicle Analysis (if vehicle profile)
        if args.profile == "vehicle":
            vehicle_analysis = stage_vehicle_analysis()
            report["stages"]["vehicle_analysis"] = {"status": "completed", "analysis": vehicle_analysis}
            if not vehicle_analysis["wheel_separation_possible"]:
                report["warnings"].append("Wheel separation not possible - wheels appear fused to body")
        else:
            report["stages"]["vehicle_analysis"] = {"status": "skipped", "reason": "generic profile"}
        
        # Stage 7/8: Validate
        validation = stage_validate(Path(args.output))
        report["stages"]["validate"] = {"status": "completed", "validation": validation}
        
        # Final report
        report["stages"]["complete"] = {"status": "completed"}
        report["final_status"] = "READY_FOR_REVIEW" if not report["errors"] else "FAILED"
        
        # Add triangle count summary
        final_stats = get_scene_stats()
        report["triangle_counts"] = {
            "pre_clean": stats_before.get("triangle_count", 0),
            "post_clean": final_stats.get("triangle_count", 0),
            "lod0": report["stages"].get("lod_generation", {}).get("lod_paths", {}).get("lod0", "master"),
        }
        
        # Material status
        report["material_status"] = "ready" if final_stats.get("material_count", 0) > 0 else "missing"
        
    except Exception as e:
        report["errors"].append(str(e))
        report["final_status"] = "FAILED"
        log(f"FATAL ERROR: {e}", "ERROR")
        traceback.print_exc()
    
    # Write report
    write_json(report_path, report)
    log(f"Report written to {report_path}")
    log(f"Final status: {report['final_status']}")

if __name__ == "__main__":
    main()