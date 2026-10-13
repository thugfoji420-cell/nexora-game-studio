# Nexora FBX deployment converter.
# Converts a processed GLB into an FBX with textures EMBEDDED, so Unity's
# native FBX importer brings the model in with colors/graphics intact even
# without a glTF importer package (glbImporterAvailable=false targets).
#
# Blender headless usage:
#   blender --background --python nexora_fbx_convert.py -- --input <in.glb> --output <out.fbx>
import argparse
import os
import sys

import bpy


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)

    bpy.ops.wm.read_factory_settings(use_empty=True)

    ext = os.path.splitext(args.input)[1].lower()
    if ext == ".glb" or ext == ".gltf":
        bpy.ops.import_scene.gltf(filepath=args.input)
    elif ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=args.input)
    elif ext == ".obj":
        bpy.ops.wm.obj_import(filepath=args.input)
    else:
        print(f"[nexora-fbx] unsupported input format: {ext}")
        sys.exit(2)

    # Drop empties left by glTF importers (armature-less rigs, stray parents).
    for obj in list(bpy.data.objects):
        if obj.type == "EMPTY" and not obj.children:
            bpy.data.objects.remove(obj, do_unlink=True)

    # Pack every image into the .blend first: embed_textures=True only embeds
    # PACKED images, and glTF import leaves images unpacked - without this the
    # FBX ships with path references only and Unity imports it colorless.
    for img in bpy.data.images:
        if img.packed_file:
            continue
        try:
            img.pack()
            print(f"[nexora-fbx] packed image {img.name}")
        except Exception as exc:  # noqa: BLE001 - keep converting, report loudly
            print(f"[nexora-fbx] FAILED to pack image {img.name}: {exc}")

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    # path_mode='COPY' is required for embedding: Blender's FBX exporter only
    # honours embed_textures when textures are copied (ABSOLUTE/AUTO ship path
    # references only, leaving Unity with a colorless model). Combined with
    # packed images this makes the FBX self-contained.
    bpy.ops.export_scene.fbx(
        filepath=args.output,
        use_selection=False,
        path_mode="COPY",
        embed_textures=True,
        mesh_smooth_type="FACE",
        add_leaf_bones=False,
        bake_space_transform=False,
        object_types={"MESH", "ARMATURE"},
    )
    size = os.path.getsize(args.output)
    print(f"[nexora-fbx] wrote {args.output} ({size} bytes)")
    if size < 1024:
        print("[nexora-fbx] output suspiciously small")
        sys.exit(3)


main()
