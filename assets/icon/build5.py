import os
# Pathfinder cairn, pass 5: chiselled hull stones settled by rigid-body simulation,
# stylised faceted shading, glass top stone, macOS icon grid (824 tile in 1024 canvas).
import bpy, bmesh, math, os, random
import numpy as np
from mathutils import Vector, Matrix
from mathutils.bvhtree import BVHTree

OUT = os.path.join(os.path.dirname(os.path.abspath(bpy.data.filepath or "build5.py")), "out")
V = globals().get("V", {})
NAME = V.get("name", "cairn")
REPORT = {}

def lin(hexs):
    h = hexs.lstrip("#"); c = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    return tuple(((x + 0.055) / 1.055) ** 2.4 if x > 0.04045 else x / 12.92 for x in c) + (1.0,)

sc = bpy.context.scene
for o in list(bpy.data.objects): bpy.data.objects.remove(o, do_unlink=True)

# ---------------- render settings
sc.render.engine = "CYCLES"
try:
    prefs = bpy.context.preferences.addons["cycles"].preferences
    prefs.compute_device_type = "METAL"; prefs.get_devices()
    for d in prefs.devices: d.use = True
    sc.cycles.device = "GPU"
except Exception:
    sc.cycles.device = "CPU"
sc.cycles.samples = V.get("samples", 256); sc.cycles.use_denoising = True
TILE = 824
sc.render.resolution_x = sc.render.resolution_y = TILE; sc.render.resolution_percentage = 100
sc.view_settings.view_transform = "Standard"; sc.view_settings.look = "None"
sc.render.image_settings.file_format = "PNG"; sc.render.image_settings.color_mode = "RGB"
world = bpy.data.worlds[0] if bpy.data.worlds else bpy.data.worlds.new("World")
sc.world = world
world.node_tree.nodes["Background"].inputs[0].default_value = lin("#3a3a66")
world.node_tree.nodes["Background"].inputs[1].default_value = 0.4

def mat_nodes(name):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    try: m.use_nodes = True
    except Exception: pass
    nt = m.node_tree
    for n in list(nt.nodes): nt.nodes.remove(n)
    return m, nt

def ramp(nt, stops, interp="LINEAR"):
    r = nt.nodes.new("ShaderNodeValToRGB"); r.color_ramp.interpolation = interp
    els = r.color_ramp.elements
    while len(els) > 1: els.remove(els[-1])
    els[0].position, els[0].color = stops[0][0], lin(stops[0][1])
    for p, c in stops[1:]:
        e = els.new(p); e.color = lin(c)
    return r

def sock(node, name, kind=None, out=False):
    for s in (node.outputs if out else node.inputs):
        if s.name == name and (kind is None or s.type == kind) and s.enabled: return s
    raise KeyError(name)

# ---------------- materials
def stone_material():
    m, nt = mat_nodes("Stone"); L = nt.links
    out = nt.nodes.new("ShaderNodeOutputMaterial"); b = nt.nodes.new("ShaderNodeBsdfPrincipled")
    L.new(b.outputs[0], out.inputs[0]); tc = nt.nodes.new("ShaderNodeTexCoord")
    n1 = nt.nodes.new("ShaderNodeTexNoise"); n1.inputs["Scale"].default_value = 2.5; n1.inputs["Detail"].default_value = 3
    L.new(tc.outputs["Object"], n1.inputs["Vector"])
    r1 = ramp(nt, [(0.38, "#d99a62"), (0.62, "#e8ae72")])  # restrained: one tone with a gentle drift
    L.new(n1.outputs["Fac"], r1.inputs[0])
    n2 = nt.nodes.new("ShaderNodeTexNoise"); n2.inputs["Scale"].default_value = 140; n2.inputs["Detail"].default_value = 1
    L.new(tc.outputs["Object"], n2.inputs["Vector"])
    r2 = ramp(nt, [(0.0, "#000000"), (0.69, "#8a8a8a")], "CONSTANT")
    L.new(n2.outputs["Fac"], r2.inputs[0])
    mix = nt.nodes.new("ShaderNodeMix"); mix.data_type = "RGBA"; mix.blend_type = "MULTIPLY"
    L.new(r2.outputs[0], sock(mix, "Factor", "VALUE")); L.new(r1.outputs[0], sock(mix, "A", "RGBA"))
    sock(mix, "B", "RGBA").default_value = lin("#7a4a2c")
    hsv = nt.nodes.new("ShaderNodeHueSaturation"); oi = nt.nodes.new("ShaderNodeObjectInfo")
    mr = nt.nodes.new("ShaderNodeMapRange"); mr.inputs["To Min"].default_value = 0.88; mr.inputs["To Max"].default_value = 1.1
    L.new(oi.outputs["Random"], mr.inputs["Value"]); L.new(mr.outputs["Result"], hsv.inputs["Value"])
    L.new(sock(mix, "Result", "RGBA", out=True), hsv.inputs["Color"]); L.new(hsv.outputs["Color"], b.inputs["Base Color"])
    n3 = nt.nodes.new("ShaderNodeTexNoise"); n3.inputs["Scale"].default_value = 30; n3.inputs["Detail"].default_value = 4
    L.new(tc.outputs["Object"], n3.inputs["Vector"])
    bump = nt.nodes.new("ShaderNodeBump"); bump.inputs["Strength"].default_value = 0.08; bump.inputs["Distance"].default_value = 0.02
    L.new(n3.outputs["Fac"], bump.inputs["Height"]); L.new(bump.outputs[0], b.inputs["Normal"])
    b.inputs["Roughness"].default_value = 0.9
    return m

def glass_material():
    m, nt = mat_nodes("Glass"); L = nt.links
    out = nt.nodes.new("ShaderNodeOutputMaterial"); b = nt.nodes.new("ShaderNodeBsdfPrincipled")
    L.new(b.outputs[0], out.inputs[0])
    b.inputs["Base Color"].default_value = lin("#dff0ff"); b.inputs["Transmission Weight"].default_value = 1.0
    b.inputs["Roughness"].default_value = 0.05; b.inputs["IOR"].default_value = 1.3
    # bright rim along the silhouette and facet edges: what makes it read as glass when tiny
    lw = nt.nodes.new("ShaderNodeLayerWeight"); lw.inputs["Blend"].default_value = V.get("glass_blend", 0.18)
    pw = nt.nodes.new("ShaderNodeMath"); pw.operation = "POWER"; pw.inputs[1].default_value = 3.0
    L.new(lw.outputs["Facing"], pw.inputs[0])
    mul = nt.nodes.new("ShaderNodeMath"); mul.operation = "MULTIPLY"; mul.inputs[1].default_value = V.get("glass_glow", 2.2)
    L.new(pw.outputs[0], mul.inputs[0]); L.new(mul.outputs[0], b.inputs["Emission Strength"])
    b.inputs["Emission Color"].default_value = lin("#e8f6ff")
    return m

def flat_material(name, hexc):
    m, nt = mat_nodes(name)
    out = nt.nodes.new("ShaderNodeOutputMaterial"); b = nt.nodes.new("ShaderNodeBsdfPrincipled")
    nt.links.new(b.outputs[0], out.inputs[0]); b.inputs["Base Color"].default_value = lin(hexc); b.inputs["Roughness"].default_value = 1.0
    return m

def sky_material():
    m, nt = mat_nodes("Sky"); L = nt.links
    out = nt.nodes.new("ShaderNodeOutputMaterial"); em = nt.nodes.new("ShaderNodeEmission")
    L.new(em.outputs[0], out.inputs[0])
    tc = nt.nodes.new("ShaderNodeTexCoord"); sep = nt.nodes.new("ShaderNodeSeparateXYZ")
    L.new(tc.outputs["Generated"], sep.inputs[0])
    r = ramp(nt, V.get("sky", [(0.0, "#ffb070"), (0.22, "#e98062"), (0.45, "#8a4f76"), (0.75, "#33305e"), (1.0, "#1d2347")]))
    L.new(sep.outputs["Y"], r.inputs[0]); L.new(r.outputs[0], em.inputs["Color"])
    return m

# ---------------- chiselled stones: convex hull of a flattened, irregular point cloud
def hull_stone(name, rx, ry, h, seed, mat, bevel=0.018):
    rnd = random.Random(seed)
    pts = []
    lobes = [(rnd.uniform(0, 6.28), rnd.uniform(0.06, 0.14)) for _ in range(3)]
    def rad(a):
        return 1 + sum(amp * math.cos((k + 2) * a + ph) for k, (ph, amp) in enumerate(lobes))
    for i in range(V.get("hull_pts", 20)):
        a = rnd.uniform(0, 2 * math.pi); z = rnd.uniform(-1, 1)
        shrink = 1 - 0.14 * abs(z) ** 1.5  # rounded-off rims
        top_shrink = 0.86 if z > 0 else 1.0
        r = rad(a) * shrink * top_shrink * rnd.uniform(0.9, 1.0)
        pts.append(Vector((rx * r * math.cos(a), ry * r * math.sin(a), h * z)))
    for i in range(6):  # flat-ish bearing faces top and bottom so the stack can rest
        a = 2 * math.pi * i / 6 + rnd.uniform(-0.3, 0.3)
        for zz, s in ((h, 0.55), (-h, 0.68)):
            pts.append(Vector((rx * s * rad(a) * math.cos(a), ry * s * rad(a) * math.sin(a), zz + rnd.uniform(-0.012, 0.012))))
    bm = bmesh.new()
    for p in pts: bm.verts.new(p)
    res = bmesh.ops.convex_hull(bm, input=bm.verts)
    for g in res.get("geom_interior", []) + res.get("geom_unused", []):
        if isinstance(g, bmesh.types.BMVert) and g.is_valid: bm.verts.remove(g)
    bmesh.ops.dissolve_limit(bm, angle_limit=math.radians(4), verts=bm.verts, edges=bm.edges)
    bmesh.ops.triangulate(bm, faces=bm.faces)
    me = bpy.data.meshes.new(name); bm.to_mesh(me); bm.free()
    for p in me.polygons: p.use_smooth = False
    ob = bpy.data.objects.new(name, me); sc.collection.objects.link(ob)
    ob.data.materials.append(mat)
    glass = mat.name == "Glass"
    bv = ob.modifiers.new("Bevel", "BEVEL"); bv.limit_method = "ANGLE"; bv.angle_limit = math.radians(25)
    bv.width = 0.05 if glass else bevel; bv.segments = 4 if glass else 2; bv.use_clamp_overlap = True
    if glass:
        for p in ob.data.polygons: p.use_smooth = True
        ss = ob.modifiers.new("Smooth", "SUBSURF"); ss.levels = ss.render_levels = 2
    return ob

stone_m, glass_m = stone_material(), glass_material()
STONES = V.get("stones", [  # rx, ry, half-height, seed, x, y, tilt_x, tilt_y, rot_z
    (1.00, 0.78, 0.17, 11, 0.00, 0.00, 0, 0, -6),
    (0.76, 0.62, 0.16, 23, 0.07, 0.02, 2, -2, 14),
    (0.60, 0.50, 0.15, 37, -0.06, -0.02, -2, 3, -18),
    (0.45, 0.38, 0.14, 41, 0.04, 0.01, 2, -2, 26),
    (0.36, 0.30, 0.14, 53, -0.01, 0.00, -3, 2, -10),  # glass
])
objs = []
z = 0.0
for i, (rx, ry, hh, seed, x, y, tx, ty, rz) in enumerate(STONES):
    glass = i == len(STONES) - 1
    ob = hull_stone(f"Stone{i}" if not glass else "GlassStone", rx, ry, hh, seed, glass_m if glass else stone_m)
    zmin = min(v.co.z for v in ob.data.vertices)
    ob.location = (x, y, z - zmin + 0.04 + 0.03 * i)
    ob.rotation_euler = (math.radians(tx), math.radians(ty), math.radians(rz))
    z = ob.location.z + max(v.co.z for v in ob.data.vertices)
    objs.append(ob)

# ground: a broad, gently domed crest
gme = bpy.data.meshes.new("Ground"); bm = bmesh.new()
bmesh.ops.create_uvsphere(bm, u_segments=128, v_segments=64, radius=1.0); bm.to_mesh(gme); bm.free()
for p in gme.polygons: p.use_smooth = True
ground = bpy.data.objects.new("Ground", gme); sc.collection.objects.link(ground)
R = 2.8; ground.scale = (R, R * 0.75, R * 0.14); ground.location = (0.0, 0.5, -R * 0.14)
ground.data.materials.append(flat_material("Ground", V.get("ground", "#1c1832")))

# ---------------- rigid body simulation
bpy.context.view_layer.update()
if sc.rigidbody_world is None:
    with bpy.context.temp_override(scene=sc):
        bpy.ops.rigidbody.world_add()
rbw = sc.rigidbody_world
rbw.substeps_per_frame = 20; rbw.solver_iterations = 40
rbw.point_cache.frame_start = 1; rbw.point_cache.frame_end = 160
if rbw.collection is None:
    rbw.collection = bpy.data.collections.new("RigidBodyWorld")

def add_rb(ob, kind, shape):
    with bpy.context.temp_override(object=ob, active_object=ob, selected_objects=[ob], selected_editable_objects=[ob]):
        bpy.ops.rigidbody.object_add(type=kind)
    rb = ob.rigid_body; rb.collision_shape = shape
    rb.friction = 1.0; rb.restitution = 0.0
    rb.use_margin = True; rb.collision_margin = 0.0
    if kind == "ACTIVE":
        rb.mass = 2.0 * ob.dimensions.x * ob.dimensions.y * ob.dimensions.z
        rb.linear_damping = 0.6; rb.angular_damping = 0.8

add_rb(ground, "PASSIVE", "MESH")
for ob in objs: add_rb(ob, "ACTIVE", "CONVEX_HULL")
start = [ob.location.copy() for ob in objs]
for f in range(1, 161):
    sc.frame_set(f)
mats = [ob.matrix_world.copy() for ob in objs]
REPORT["drift"] = [round((m.translation - s).xy.length, 3) for m, s in zip(mats, start)]
REPORT["drop"] = [round(s.z - m.translation.z, 3) for m, s in zip(mats, start)]
# apply visual transform, drop the simulation
for ob, m in zip(objs, mats):
    with bpy.context.temp_override(object=ob, active_object=ob, selected_objects=[ob], selected_editable_objects=[ob]):
        bpy.ops.rigidbody.object_remove()
    ob.matrix_world = m
with bpy.context.temp_override(object=ground, active_object=ground, selected_objects=[ground], selected_editable_objects=[ground]):
    bpy.ops.rigidbody.object_remove()
with bpy.context.temp_override(scene=sc):
    bpy.ops.rigidbody.world_remove()
sc.frame_set(1)
bpy.context.view_layer.update()

# ---------------- intersection check (true penetration, not mere contact)
dg = bpy.context.evaluated_depsgraph_get()
def bvh(ob):
    me = ob.evaluated_get(dg).to_mesh()
    vs = [ob.matrix_world @ v.co for v in me.vertices]; fs = [list(p.vertices) for p in me.polygons]
    ob.evaluated_get(dg).to_mesh_clear()
    return BVHTree.FromPolygons(vs, fs), vs
def inside_count(vs, ob):
    # vertices of one body lying inside another convex body by more than 2 mm
    me = ob.data; mw = ob.matrix_world; n = 0
    planes = [(mw @ p.center, (mw.to_3x3() @ p.normal).normalized()) for p in me.polygons]
    for v in vs:
        if all((v - c).dot(nn) < -0.002 for c, nn in planes): n += 1
    return n
pairs = {}
B = {ob.name: bvh(ob) for ob in objs}
for i in range(len(objs)):
    for j in range(len(objs)):
        if i != j:
            k = inside_count(B[objs[i].name][1], objs[j])
            if k: pairs[f"{objs[i].name} in {objs[j].name}"] = k
# ground: stone vertices below the ground surface
gb = bvh(ground)[0]
below = 0
for ob in objs:
    for v in B[ob.name][1]:
        hit = gb.ray_cast(v + Vector((0, 0, 5)), Vector((0, 0, -1)))
        if hit[0] is not None and v.z < hit[0].z - 0.002: below += 1
REPORT["penetrating_vertices"] = pairs; REPORT["below_ground"] = below

# ---------------- camera: centred on the settled cairn
pts = [ob.matrix_world @ Vector(c) for ob in objs for c in ob.bound_box]
el = math.radians(V.get("elev", 9))
fwd = Vector((0, math.cos(el), -math.sin(el))); up = Vector((0, math.sin(el), math.cos(el))); right = Vector((1, 0, 0))
xs = [p.dot(right) for p in pts]; ys = [p.dot(up) for p in pts]
cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
hgt = max(ys) - min(ys); wid = max(xs) - min(xs)
ortho = max(hgt / V.get("fill_h", 0.70), wid / V.get("fill_w", 0.80))
cy -= ortho * V.get("lift", -0.07)  # sit the cairn a touch low so the ground shows as a narrow band
cd = bpy.data.cameras.new("Cam"); cd.type = "ORTHO"; cd.ortho_scale = ortho
cam = bpy.data.objects.new("Cam", cd); sc.collection.objects.link(cam)
cam.matrix_world = Matrix.Translation(right * cx + up * cy - fwd * 30) @ Matrix.Rotation(math.pi / 2 - el, 4, "X")
sc.camera = cam
pme = bpy.data.meshes.new("Sky"); bm = bmesh.new(); bmesh.ops.create_grid(bm, x_segments=1, y_segments=1, size=1.0); bm.to_mesh(pme); bm.free()
sky = bpy.data.objects.new("Sky", pme); sc.collection.objects.link(sky); sky.data.materials.append(sky_material())
sky.parent = cam; sky.matrix_parent_inverse = Matrix(); sky.location = (0, 0, -60); sky.scale = (ortho / 2 * 1.02, ortho / 2 * 1.02, 1)
sky.visible_diffuse = False; sky.visible_shadow = False

def area(name, loc, target, size, power, hexc):
    ld = bpy.data.lights.new(name, "AREA"); ld.size = size; ld.energy = power; ld.color = lin(hexc)[:3]
    ob = bpy.data.objects.new(name, ld); sc.collection.objects.link(ob); ob.location = loc
    ob.rotation_euler = (Vector(target) - Vector(loc)).to_track_quat("-Z", "Y").to_euler()
area("Key", (-4, -4, 8), (0, 0, 0.6), 2.5, V.get("key", 1500), "#fff0dc")
area("Fill", (6, -5, 1.5), (0, 0, 0.6), 6, V.get("fill", 160), "#a9b6ff")
area("Rim", (2.0, 5, 3.0), (0, 0, 0.8), 2.5, V.get("rim", 1400), "#ffb27a")

# ---------------- check view from the side, then the icon render
chk = bpy.data.cameras.new("CheckCam"); chk.type = "ORTHO"; chk.ortho_scale = ortho
cc = bpy.data.objects.new("CheckCam", chk); sc.collection.objects.link(cc)
cc.location = (30, 0, cy); cc.rotation_euler = (math.radians(90), 0, math.radians(90))
if V.get("check", True):
    sc.camera = cc; sc.cycles.samples = 24
    sc.render.filepath = os.path.join(OUT, f"_{NAME}-side.png"); bpy.ops.render.render(write_still=True)
    sc.camera = cam; sc.cycles.samples = V.get("samples", 256)
raw = os.path.join(OUT, f"_{NAME}-raw.png")
sc.render.filepath = raw
if V.get("render", True):
    bpy.ops.render.render(write_still=True)
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(OUT, f"{NAME}.blend"))

# ---------------- compose onto the macOS icon grid: 824 tile at 100px in a 1024 canvas
if V.get("render", True):
    img = bpy.data.images.load(raw, check_existing=False)
    art = np.array(img.pixels[:], dtype=np.float32).reshape(TILE, TILE, 4)
    C, OFF, RAD = 1024, 100, 185.4
    yy, xx = np.mgrid[0:C, 0:C] + 0.5
    lo, hi = OFF + RAD, OFF + TILE - RAD
    d = np.hypot(xx - np.clip(xx, lo, hi), yy - np.clip(yy, lo, hi)) - RAD
    inside = (xx >= OFF) & (xx <= OFF + TILE) & (yy >= OFF) & (yy <= OFF + TILE)
    mask = np.clip(0.5 - d, 0, 1) * inside
    # soft drop shadow under the tile (pixels are bottom-up, so "down" is -y)
    def blur(a, r):
        for _ in range(3):
            k = 2 * r + 1; p = np.pad(a, ((r, r), (0, 0)), mode="edge"); c = np.cumsum(p, 0); c = np.vstack([np.zeros((1, a.shape[1])), c])
            a = (c[k:] - c[:-k]) / k
            p = np.pad(a, ((0, 0), (r, r)), mode="edge"); c = np.cumsum(p, 1); c = np.hstack([np.zeros((a.shape[0], 1)), c])
            a = (c[:, k:] - c[:, :-k]) / k
        return a
    sh = np.roll(mask, -10, axis=0); sh = blur(sh, 7) * 0.30
    canvas = np.zeros((C, C, 4), np.float32)
    canvas[..., 3] = sh  # black shadow
    tile = np.zeros((C, C, 3), np.float32); tile[OFF:OFF + TILE, OFF:OFF + TILE] = art[..., :3]
    a = mask[..., None]
    out_a = a[..., 0] + canvas[..., 3] * (1 - a[..., 0])
    out_rgb = (tile * a) / np.maximum(out_a[..., None], 1e-6)
    canvas[..., :3] = out_rgb; canvas[..., 3] = out_a
    def save(arr, path):
        n = arr.shape[0]; im = bpy.data.images.new(os.path.basename(path), n, n, alpha=True)
        im.pixels.foreach_set(arr.ravel()); im.filepath_raw = path; im.file_format = "PNG"; im.save(); bpy.data.images.remove(im)
    save(canvas, os.path.join(OUT, f"{NAME}.png"))
    pm = canvas.copy(); pm[..., :3] *= pm[..., 3:4]; size = C
    while size > 16:
        pm = pm.reshape(size // 2, 2, size // 2, 2, 4).mean(axis=(1, 3)); size //= 2
        if size in (512, 32, 16):
            o = pm.copy(); al = o[..., 3:4]; o[..., :3] = np.where(al > 0, o[..., :3] / np.maximum(al, 1e-6), 0)
            save(o, os.path.join(OUT, f"{NAME}-{size}.png"))
result = REPORT
