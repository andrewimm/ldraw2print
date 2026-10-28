#!/usr/bin/env python3
"""Generate the self-contained LDraw test library in tests/fixtures/ldraw.

These are original, minimal files written for ldraw2print's tests (not
copies of the official LDraw library). Run: python3 tests/fixtures/gen_fixtures.py
"""
import math, os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ldraw")
SEG = 16
ANG = [2 * math.pi * k / SEG for k in range(SEG + 1)]
C = [round(math.cos(a), 4) for a in ANG]
S = [round(math.sin(a), 4) for a in ANG]


def f(x):
    s = f"{x:.4f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def pt(*p):
    return " ".join(f(v) for v in p)


def write(rel, lines):
    path = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        fh.write("\n".join(lines) + "\n")


def header(desc, name, kind):
    return [f"0 {desc}", f"0 Name: {name}", "0 Author: ldraw2print test fixtures",
            f"0 !LDRAW_ORG {kind}", "0 !LICENSE MIT, part of ldraw2print's test suite", ""]


# Unit cylinder wall, y 0..1, radius 1, normals outward (CCW).
cyl = header("Cylinder 1.0 (test)", "4-4cyli.dat", "Primitive") + ["0 BFC CERTIFY CCW", ""]
for k in range(SEG):
    a0, a1 = (C[k], 0, S[k]), (C[k + 1], 0, S[k + 1])
    # p0 (y=0,a0) -> q0 (y=1,a0) -> q1 -> p1 is outward for increasing angle.
    cyl.append(f"4 16 {pt(C[k],0,S[k])} {pt(C[k],1,S[k])} {pt(C[k+1],1,S[k+1])} {pt(C[k+1],0,S[k+1])}")
write("p/4-4cyli.dat", cyl)

# Unit disc in y=0, normal -Y (up), like a stud top.
disc = header("Disc 1.0 (test)", "4-4disc.dat", "Primitive") + ["0 BFC CERTIFY CCW", ""]
for k in range(SEG):
    disc.append(f"3 16 0 0 0 {pt(C[k],0,S[k])} {pt(C[k+1],0,S[k+1])}")
write("p/4-4disc.dat", disc)

# Unit box [0,1]^3, outward CCW.
V = [(0,0,0),(1,0,0),(1,1,0),(0,1,0),(0,0,1),(1,0,1),(1,1,1),(0,1,1)]
FACES = [(0,3,2,1),(4,5,6,7),(0,1,5,4),(2,3,7,6),(1,2,6,5),(0,4,7,3)]
box = header("Box (test)", "testbox.dat", "Part") + ["0 BFC CERTIFY CCW", ""]
for fc in FACES:
    box.append("4 16 " + " ".join(pt(*V[i]) for i in fc))
write("parts/testbox.dat", box)

# Same box written clockwise.
cw = header("Box clockwise (test)", "testboxcw.dat", "Part") + ["0 BFC CERTIFY CW", ""]
for fc in FACES:
    cw.append("4 16 " + " ".join(pt(*V[i]) for i in reversed(fc)))
write("parts/testboxcw.dat", cw)

# Uncertified box with three faces reversed.
nc = header("Box uncertified, inconsistent (test)", "testnocert.dat", "Part") + ["0 BFC NOCERTIFY", ""]
for i, fc in enumerate(FACES):
    order = reversed(fc) if i % 2 else fc
    nc.append("4 16 " + " ".join(pt(*V[j]) for j in order))
write("parts/testnocert.dat", nc)

# Asymmetric closed wedge subpart (triangular prism), outward CCW.
W = [(0,0,0),(4,0,0),(0,0,2),(0,-3,0),(4,-3,0),(0,-3,2)]
wedge = header("Wedge subpart (test)", "s\\testwedge.dat", "Subpart") + ["0 BFC CERTIFY CCW", ""]
def tri_out(a, b, c, centroid):
    import operator
    pa, pb, pc = W[a], W[b], W[c]
    e1 = [pb[i]-pa[i] for i in range(3)]; e2 = [pc[i]-pa[i] for i in range(3)]
    n = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]]
    out = [pa[i]-centroid[i] for i in range(3)]
    return (a, b, c) if sum(n[i]*out[i] for i in range(3)) > 0 else (a, c, b)
cen = [sum(p[i] for p in W)/6 for i in range(3)]
for t in [(0,1,2),(3,4,5)]:
    a,b,c = tri_out(*t, cen); wedge.append(f"3 16 {pt(*W[a])} {pt(*W[b])} {pt(*W[c])}")
for q in [(0,1,4,3),(1,2,5,4),(2,0,3,5)]:
    a,b,c = tri_out(q[0], q[1], q[2], cen)
    order = q if (a,b,c) == (q[0],q[1],q[2]) else tuple(reversed(q))
    wedge.append("4 16 " + " ".join(pt(*W[i]) for i in order))
write("parts/s/testwedge.dat", wedge)

# Mirrored pair + coloured reference.
write("parts/testmirror.dat", header("Mirrored wedges (test)", "testmirror.dat", "Part") + [
    "0 BFC CERTIFY CCW", "",
    "1 16 10 0 0 1 0 0 0 1 0 0 0 1 s\\testwedge.dat",
    "1 4 -10 0 0 -1 0 0 0 1 0 0 0 1 s\\testwedge.dat",
])

# Hollow box: outer box, inner cavity via INVERTNEXT.
write("parts/testhollow.dat", header("Hollow box (test)", "testhollow.dat", "Part") + [
    "0 BFC CERTIFY CCW", "",
    "1 16 0 0 0 10 0 0 0 10 0 0 0 10 testbox.dat",
    "0 BFC INVERTNEXT",
    "1 16 2 2 2 6 0 0 0 6 0 0 0 6 testbox.dat",
])

# Synthetic minifig hat: solid 16-gon prism r=14, 10 LDU tall, with a head
# stud socket r=6, 4 LDU deep, opening at y=0 (LDraw headwear convention).
hat = header("Test Minifig Hat Plain", "testhat.dat", "Part") + [
    "0 BFC CERTIFY CCW", "",
    "0 // socket wall (faces inward)",
    "0 BFC INVERTNEXT",
    "1 16 0 0 0 6 0 0 0 -4 0 0 0 6 4-4cyli.dat",
    "0 // socket ceiling, faces down into the socket",
    "1 16 0 -4 0 6 0 0 0 -1 0 0 0 6 4-4disc.dat",
    "0 // crown wall and top",
    "1 16 0 -10 0 14 0 0 0 10 0 0 0 14 4-4cyli.dat",
    "1 16 0 -10 0 14 0 0 0 1 0 0 0 14 4-4disc.dat",
    "0 // underside annulus r 6..14, written clockwise to exercise BFC CW",
    "0 BFC CW",
]
for k in range(SEG):
    i0 = (6*C[k], 0, 6*S[k]); i1 = (6*C[k+1], 0, 6*S[k+1])
    o0 = (14*C[k], 0, 14*S[k]); o1 = (14*C[k+1], 0, 14*S[k+1])
    # CCW with normal +Y (down) is i0,i1,o1,o0 ; write reversed for CW.
    hat.append("4 16 " + " ".join(pt(*p) for p in (o0, o1, i1, i0)))
hat += ["0 BFC CCW"]
write("parts/testhat.dat", hat)

# Hat with the stud socket built from raw quads (no primitives).
raw = header("Test Hat Raw Socket", "testhatraw.dat", "Part") + ["0 BFC CERTIFY CCW", ""]
for k in range(SEG):
    for (r, y0, y1, inward) in [(6, 0, -4, True), (14, 0, -10, False)]:
        p0 = (r*C[k], y0, r*S[k]); p1 = (r*C[k+1], y0, r*S[k+1])
        q0 = (r*C[k], y1, r*S[k]); q1 = (r*C[k+1], y1, r*S[k+1])
        # outward for (y0 > y1, i.e. going up): p0,p1,q1,q0 ; inward reversed
        quad = (p0, p1, q1, q0) if not inward else (q0, q1, p1, p0)
        raw.append("4 16 " + " ".join(pt(*p) for p in quad))
    raw.append(f"3 16 0 -4 0 {pt(6*C[k+1],-4,6*S[k+1])} {pt(6*C[k],-4,6*S[k])}")
    raw.append(f"3 16 0 -10 0 {pt(14*C[k],-10,14*S[k])} {pt(14*C[k+1],-10,14*S[k+1])}")
    i0 = (6*C[k], 0, 6*S[k]); i1 = (6*C[k+1], 0, 6*S[k+1])
    o0 = (14*C[k], 0, 14*S[k]); o1 = (14*C[k+1], 0, 14*S[k+1])
    raw.append("4 16 " + " ".join(pt(*p) for p in (i0, i1, o1, o0)))
write("parts/testhatraw.dat", raw)

# Two identical head-stud sockets side by side: auto-selection must refuse
# (exit code 4) and ask for --connector-candidate.
two = header("Test Two Sockets", "testtwosockets.dat", "Part") + ["0 BFC CERTIFY CCW", ""]
for x in (-20, 20):
    two += ["0 BFC INVERTNEXT", f"1 16 {x} 0 0 6 0 0 0 -4 0 0 0 6 4-4cyli.dat",
            f"1 16 {x} -4 0 6 0 0 0 -1 0 0 0 6 4-4disc.dat"]
write("parts/testtwosockets.dat", two)

# Broken references.
write("parts/testmissing.dat", header("Missing reference (test)", "testmissing.dat", "Part") + [
    "0 BFC CERTIFY CCW", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 testbox.dat",
    "1 16 0 0 0 1 0 0 0 1 0 0 0 1 doesnotexist.dat"])
write("parts/testloopa.dat", header("Loop A (test)", "testloopa.dat", "Part") + [
    "1 16 0 0 0 1 0 0 0 1 0 0 0 1 testloopb.dat"])
write("parts/testloopb.dat", header("Loop B (test)", "testloopb.dat", "Part") + [
    "1 16 0 0 0 1 0 0 0 1 0 0 0 1 testloopa.dat"])

write("LDConfig.ldr", [
    "0 LDraw.org Configuration File (test subset)",
    "0 !COLOUR Black CODE 0 VALUE #1B2A34 EDGE #808080",
    "0 !COLOUR Red CODE 4 VALUE #B40000 EDGE #333333",
    "0 !COLOUR Main_Colour CODE 16 VALUE #FFFF80 EDGE #333333",
    "0 !COLOUR Edge_Colour CODE 24 VALUE #7F7F7F EDGE #333333",
    "0 !COLOUR Light_Bluish_Grey CODE 71 VALUE #A0A5A9 EDGE #333333",
    "0 !COLOUR Trans_Clear CODE 47 VALUE #FCFCFC EDGE #C3C3C3 ALPHA 128",
])
print("fixtures written to", ROOT)
