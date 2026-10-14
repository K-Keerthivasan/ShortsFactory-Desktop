# Shorts Factory Desktop app icon: a 9:16 frame holding a play head, two side bars, on a red tile.
import sys
N=1024; SS=3
def inside_rrect(x,y,x0,y0,x1,y1,r):
    if x<x0 or x>x1 or y<y0 or y>y1: return False
    cx=min(max(x,x0+r),x1-r); cy=min(max(y,y0+r),y1-r)
    return (x-cx)**2+(y-cy)**2<=r*r
def in_tri(x,y,a,b,c):
    def s(p,q,r): return (p[0]-r[0])*(q[1]-r[1])-(q[0]-r[0])*(p[1]-r[1])
    d1=s((x,y),a,b); d2=s((x,y),b,c); d3=s((x,y),c,a)
    neg=(d1<0) or (d2<0) or (d3<0); pos=(d1>0) or (d2>0) or (d3>0)
    return not (neg and pos)
BG=(0xff,0x2e,0x3f); INK=(255,255,255); DARK=(0x15,0x15,0x14)
def colour(x,y):
    if not inside_rrect(x,y,64,64,960,960,200): return None
    if inside_rrect(x,y,362,190,662,834,64):
        if in_tri(x,y,(458,394),(458,630),(620,512)): return BG
        return INK
    if inside_rrect(x,y,232,342,286,682,27) or inside_rrect(x,y,738,342,792,682,27):
        return (255,214,219)
    return BG
out=bytearray()
for py in range(N):
    for px in range(N):
        r=g=b=a=0
        for sy in range(SS):
            for sx in range(SS):
                c=colour(px+(sx+.5)/SS, py+(sy+.5)/SS)
                if c: r+=c[0]; g+=c[1]; b+=c[2]; a+=255
        n=SS*SS
        if a: out+=bytes((r*255//a if a else 0, g*255//a if a else 0, b*255//a if a else 0, a//n))
        else: out+=b'\0\0\0\0'
open('icon.rgba','wb').write(out)
