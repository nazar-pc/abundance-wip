import sys,re,collections
dis,prof=sys.argv[1],sys.argv[2]
counts={};taken={}
for l in open(prof):
    a,c,t=l.split(); counts[int(a,16)]=int(c); taken[int(a,16)]=int(t)
funcs=collections.OrderedDict(); cur=None; lines=[]
for l in open(dis):
    m=re.match(r'^([0-9a-f]+) <(.*)>:$',l)
    if m: cur=m.group(2); funcs[cur]=[0,0]; continue
    m=re.match(r'^ +([0-9a-f]+):\s+([0-9a-f ]+?)\s{2,}(.*)$',l)
    if m and cur:
        a=int(m.group(1),16); c=counts.get(a,0); funcs[cur][0]+=c; funcs[cur][1]+=1
        lines.append((cur,a,c,taken.get(a,0),m.group(3).strip(),len(m.group(2).replace(' ',''))//2))
total=sum(v[0] for v in funcs.values())
out=open(prof+'.funcs','w')
for k,v in sorted(funcs.items(),key=lambda kv:-kv[1][0]):
    if v[0]: out.write(f"{v[0]:12d} {100*v[0]/total:6.2f}% {v[1]:5d} {k}\n")
out=open(prof+'.annot','w')
for cur,a,c,t,ins,sz in lines:
    out.write(f"{cur:28s} {a:6x} {c:11d} {t:11d} {sz}  {ins}\n")
print(total)
