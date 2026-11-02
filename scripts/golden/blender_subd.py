# Records Catmull-Clark reference output from Blender for Vernon's golden tests.
# Usage: blender -b --factory-startup --python blender_subd.py -- <input.obj> <out_dir> <levels>
# Writes refined.txt (plain refinement) and limit.txt (vertices on the limit
# surface), one "x y z" line per vertex, plus faces.txt with the face count.
import sys
import bpy

argv = sys.argv[sys.argv.index("--") + 1:]
src, out_dir, levels = argv[0], argv[1], int(argv[2])

for limit, name in ((False, "refined"), (True, "limit")):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    # Keep the file's own axes: no Y-up to Z-up conversion.
    bpy.ops.wm.obj_import(filepath=src, forward_axis="Y", up_axis="Z")
    obj = bpy.context.selected_objects[0]
    bpy.context.view_layer.objects.active = obj
    mod = obj.modifiers.new("subd", "SUBSURF")
    mod.subdivision_type = "CATMULL_CLARK"
    mod.levels = levels
    mod.render_levels = levels
    mod.use_limit_surface = limit
    mod.boundary_smooth = "PRESERVE_CORNERS"
    mod.use_creases = False
    bpy.ops.object.modifier_apply(modifier=mod.name)
    mesh = obj.data
    mw = obj.matrix_world
    with open(f"{out_dir}/{name}.txt", "w") as f:
        for v in mesh.vertices:
            p = mw @ v.co
            f.write(f"{p.x:.9f} {p.y:.9f} {p.z:.9f}\n")
    with open(f"{out_dir}/{name}_faces.txt", "w") as f:
        f.write(f"{len(mesh.polygons)}\n")
print("GOLDEN_DONE", bpy.app.version_string)
