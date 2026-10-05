# Extract the first RT_GROUP_ICON of a PE file to an .ico: exe_icon.py <exe> <out.ico> (used by bundle.sh)
import struct,sys
d=open(sys.argv[1],'rb').read()
pe=struct.unpack_from('<I',d,0x3c)[0]
nsec=struct.unpack_from('<H',d,pe+6)[0]; osz=struct.unpack_from('<H',d,pe+20)[0]
opt=pe+24; magic=struct.unpack_from('<H',d,opt)[0]
dd=opt+(96 if magic==0x10b else 112)
rva,_=struct.unpack_from('<II',d,dd+16)
secs=[struct.unpack_from('<IIII',d,opt+osz+i*40+8) for i in range(nsec)]
def off(r):
    for vs,va,rs,rp in secs:
        if va<=r<va+max(vs,rs): return r-va+rp
if not rva: print("no rsrc"); sys.exit(1)
base=off(rva)
def ents(o):
    n=struct.unpack_from('<HH',d,o+12); out=[]
    for i in range(sum(n)):
        nid,x=struct.unpack_from('<II',d,o+16+i*8); out.append((nid,x))
    return out
def leaf(x):
    o=base+(x&0x7fffffff)
    while x&0x80000000:
        e=ents(o)[0]; x=e[1]; o=base+(x&0x7fffffff)
    r,s,_,_=struct.unpack_from('<IIII',d,o); return d[off(r):off(r)+s]
types={t:x for t,x in ents(base)}
print(sorted(types))
if 14 not in types: sys.exit(1)
icons={}
for i,x in ents(base+(types[3]&0x7fffffff)): icons[i]=leaf(x) if 3 in types else None
g=ents(base+(types[14]&0x7fffffff))
gid,gx=g[0]; grp=leaf(gx) if not gx&0x80000000 else None
o=base+(gx&0x7fffffff); e=ents(o)[0]; grp=leaf(e[1])
_,_,cnt=struct.unpack_from('<HHH',grp,0)
hdr=struct.pack('<HHH',0,1,cnt); dirs=b''; data=b''; pos=6+16*cnt
for i in range(cnt):
    w,h,c,r,pl,bc,sz,iid=struct.unpack_from('<BBBBHHIH',grp,6+14*i)
    img=icons[iid]; dirs+=struct.pack('<BBBBHHII',w,h,c,r,pl,bc,len(img),pos+len(data)); data+=img
    print(w,h,bc)
open(sys.argv[2],'wb').write(hdr+dirs+data)
