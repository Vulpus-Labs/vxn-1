#!/usr/bin/env python3
"""Translate every DX7 factory ROM voice into a vxn-2 factory preset TOML.

Folded into the repo for E026/0126 so factory-bank regeneration is reproducible.

Inputs (NOT committed — Yamaha ROM data; supply your own, see tools/README.md):
  the eight DX7 factory cartridge dumps `rom{1..4}{a,b}.syx`, looked up in
  `$VXN2_DX7_ROMS` (or `tools/roms/`, falling back to `/tmp`).

Output: `crates/vxn2-engine/presets/factory/<Category>/<name>.toml`, computed
relative to this script so it works from any checkout.

Master-volume (E026/0126): the per-patch level is set from a *log-curve-aware*
carrier-loudness estimate — sum of `2^((OL-99)/8)` over the algorithm's carriers
(the DX7 log amplitude of each carrier's output level, ADR 0007) — gain-matched
to `TARGET_PEAK_DB`. This supersedes the old carrier-*count* heuristic, which
assumed near-full carriers under the retired square curve and left the bank too
quiet once the log curve landed. The estimate ignores FM brightness, EG sustain,
and feedback, so it is a calibrated *starting point*; the final gain-match is an
ear / measured-RMS pass (see tools/README.md).
"""
import os, re, sys, math
import dx7decode as dec

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.normpath(os.path.join(
    SCRIPT_DIR, "..", "crates", "vxn2-engine", "presets", "factory"))
BANKS = ["rom1a", "rom1b", "rom2a", "rom2b", "rom3a", "rom3b", "rom4a", "rom4b"]
SKIP = {"TUB BELLS", "BASS    1", "E.ORGAN 1", "HARPSICH 1", "MARIMBA"}  # hand-made already


def rom_dir():
    """Where the `.syx` ROM dumps live. `$VXN2_DX7_ROMS`, else `tools/roms/`,
    else `/tmp` (the legacy scratch location)."""
    env = os.environ.get("VXN2_DX7_ROMS")
    if env:
        return env
    local = os.path.join(SCRIPT_DIR, "roms")
    if os.path.isdir(local):
        return local
    return "/tmp"


# DX7 carriers per algorithm (1-based op numbers) — for loudness scaling only.
CARRIERS = {
 1:[1,3],2:[1,3],3:[1,4],4:[1,4],5:[1,3,5],6:[1,3,5],7:[1,3],8:[1,3],
 9:[1,3],10:[1,4],11:[1,4],12:[1,3],13:[1,3],14:[1,3],15:[1,3],16:[1],
 17:[1],18:[1],19:[1,4,5],20:[1,2,4],21:[1,2,4,5],22:[1,3,4,5],23:[1,2,4,5],
 24:[1,2,3,4,5],25:[1,2,3,4,5],26:[1,2,4],27:[1,2,4],28:[1,3,6],29:[1,2,3,5],
 30:[1,2,3,6],31:[1,2,3,4,5],32:[1,2,3,4,5,6]}

LFO_SHAPE = {0:"Tri",1:"Saw-",2:"Saw+",3:"Pulse",4:"Sine",5:"S&H"}
KS_CURVE  = {0:"neg-lin",1:"neg-exp",2:"pos-exp",3:"pos-lin"}  # DX7 idx -> vxn-2 name
PITCHMODSENS = [0,10,20,33,55,92,153,255]                      # DX7 pms->sens (Dexed /255)

# Loudness gain-match target (dBFS). A single full-output carrier (estimate 1.0)
# lands here; the loudest patches (a 6-carrier organ at OL 99, estimate 6) sit
# ~15.6 dB below it. Chosen so that loudest case ≈ -18.7 dBFS (no clip) while
# typical 1-2 carrier voices sit in a comfortable -3..-9 dBFS band.
TARGET_PEAK_DB = -3.0
MV_MIN, MV_MAX = -24.0, 6.0


def carrier_loudness(v):
    """Log-curve carrier-output-level sum for algorithm `v['algo']`: the DX7
    log amplitude `2^((OL-99)/8)` of each carrier's output level, summed. A
    coarse proxy for the patch's full-EG peak loudness under the log curve."""
    cars = CARRIERS.get(v['algo'], [1])
    s = 0.0
    for ci in cars:
        ol = v['ops'][ci - 1]['out']
        s += 2.0 ** ((min(ol, 99) - 99) / 8.0)
    return max(s, 1e-4)


def master_volume(v):
    est = carrier_loudness(v)
    mv = TARGET_PEAK_DB - 20.0 * math.log10(est)
    return round(max(MV_MIN, min(MV_MAX, mv)), 2)


# ── category from voice name (ordered keyword rules) ───────────────────────
CATRULES = [
 ("Bass",   ["BASS"]),
 ("Brass",  ["BRASS","TRUMPE","TROMB","HORN","TUBA"]),
 ("Strings",["STRING","ORCH","CELLO","VIOLIN","VIOLA","ENSEMBL"]),
 ("Organ",  ["ORGAN","PIPE","CALIOPE","ACCORD","REED"]),
 ("Keys",   ["PIANO","E.PIANO","EPIANO","CLAV","HARPSI","CELESTE","TOY","RHODE","WURLI"]),
 ("Bell",   ["BELL","CHIME","GONG","TUB","GLOK","GLOCK","CARILL"]),
 ("Mallet", ["VIBE","MARIMBA","XYLO","STEEL","LOG","KALIMBA","MUSIC BOX","KOTO"]),
 ("Perc",   ["TIMPANI","DRUM","BLOCK","COW","CLOCK","KNOCK","SNARE","TOM","CLAP","WOOD","CLAVE"]),
 ("Wind",   ["FLUTE","PICCOLO","OBOE","CLARIN","SAX","BASSOON","RECORD","WHIST","SHAKU","OCARIN",
             "HRMNCA","HARMNCA","HARMONIC"]),
 ("Pluck",  ["GUITAR","GUIT","SITAR","BANJO","LUTE","HARP","HARPE","PLUCK","ZITHER","MANDOL"]),
 ("Voice",  ["VOICE","CHOIR","VOX","VOCAL","AHH","OOH"]),
 ("Lead",   ["LEAD","SYN-LEAD","SYNLEAD","SOLO"]),
 ("Pad",    ["PAD","SWEEP","SPACE","AMBIEN"]),
 ("FX",     ["TRAIN","TAKE OFF","TAKEOFF","HELICO","WIND","RAIN","SURF","JET","BIRD","INSECT",
             "LASER","BOMB","EXPLO","SUB","WHISL","REFS","GUNSHOT","TURTLE","GROWL","SCRATCH",
             "NOISE","SFX","ZAP","UFO"]),
]
def category(name):
    u = name.upper()
    for cat, keys in CATRULES:
        if any(k in u for k in keys):
            return cat
    return "Synth"

def sanitize(name):
    s = re.sub(r"\s+"," ", name.strip())
    s = re.sub(r"[^A-Za-z0-9 _+\-.]","", s)
    return s or "Untitled"

# ── ratio: DX7 coarse/fine (* optional transpose factor) -> num/denom/fine ──
def ratio_to_ndf(R):
    best=None
    for denom in range(1,9):
        x=R*denom
        num=round(x)
        if num<1: num=1
        if num>32: num=32
        fine=round((x-num)*100)
        if fine<-99 or fine>99: continue
        err=abs((num+fine/100)/denom - R)
        # Prefer clean simple fractions, but penalise large denominators so we
        # don't render e.g. 5.8 as 29/5 when 6 - 0.20 (denom 1) is far saner.
        # 1/2, 1/4, 3/2, 4/3 still win; high-denom near-exact junk loses.
        key=(round(err,6), abs(fine)+10*(denom-1), denom)
        if best is None or key<best[0]:
            best=(key,num,denom,fine)
    if best is None:  # R exceeds vxn-2 max ratio -> clamp as close as fine allows
        num=min(32,max(1,int(R)))
        fine=max(-99,min(99,round((R-num)*100)))
        return num,1,fine
    _,num,denom,fine=best
    return num,denom,fine

def emit(v, used_paths):
    name = v['name'].strip()
    disp = sanitize(name)
    cat = category(name)
    L=[]
    L.append("schema = 1\n")
    L.append("[meta]")
    L.append(f'name = "{disp}"')
    L.append('author = "Vulpus Labs"')
    L.append(f'category = "{cat}"')
    delta = v['transpose']-24
    notes=[f"Translated from DX7 factory ROM '{name}': algo {v['algo']}, feedback {v['feedback']}."]
    if delta: notes.append(f"DX7 transpose {delta:+d} st baked into ratios.")
    L.append(f'comment = "{" ".join(notes)}"')
    L.append("")
    L.append("[params]")
    L.append(f"algo = {v['algo']}")
    # DX7 fb 0-7 -> vxn-2 0-7, scaled down: the FB table is geometric (7->1.0,
    # 6->0.5, ...), so x0.7 on the param cuts high feedback hard (noise) while
    # barely touching low feedback. (FEEDBACK_SCALE tuning knob.)
    if v['feedback']:
        L.append(f"feedback = {round(v['feedback']*0.7,2)}")
    L.append("")

    tfac = 2**(delta/12) if delta else 1.0
    for i,o in enumerate(v['ops'],1):
        L.append(f"# op{i}")
        det = round((o['detune']-7)*0.7)   # DX7 detune param -> cents (tamed x0.7)
        if o['mode']:  # fixed
            hz = 10**((o['coarse']&3)+o['fine']/100.0)
            L.append(f'op{i}-ratio-mode = "Fixed"')
            L.append(f"op{i}-fixed-hz = {round(hz,1)}")
        else:
            base = 0.5 if o['coarse']==0 else float(o['coarse'])
            F = o['fine']
            if 0 < F <= 10:
                # Small DX7 fine = unison/detune intent. vxn-2 `fine` is octave-
                # scaled (1 unit ~= 17c at num=1), so a faithful copy beats harshly.
                # Reinterpret 1 fine-unit -> 1 cent of op-detune (~17x gentler) for a
                # musical chorus; keep the clean harmonic ratio. (User-validated.)
                num,denom,fine = ratio_to_ndf(base*tfac)
                det += F
            else:
                # no fine, or larger fine = a genuine interval (1.5, 4/3, ...) -> ratio
                num,denom,fine = ratio_to_ndf(base*(1+F/100.0)*tfac)
            L.append(f"op{i}-num = {num}")
            if denom!=1: L.append(f"op{i}-denom = {denom}")
            if fine:     L.append(f"op{i}-fine = {fine}")
        det = max(-100, min(100, det))
        if det: L.append(f"op{i}-detune = {det}")
        L.append(f"op{i}-level = {o['out']}")
        L.append(f"op{i}-vel-sens = {o['kvs']}")
        # envelope (4 rate + 4 level), DX7 1:1
        for k,r in enumerate(o['eg_r'],1): L.append(f"op{i}-eg-r{k} = {r}")
        for k,l in enumerate(o['eg_l'],1): L.append(f"op{i}-eg-l{k} = {l}")
        # keyboard scaling
        if o['rs']!=2: L.append(f"op{i}-ks-rate = {o['rs']}")
        bp = o['brk']+21
        if o['ld']>0 or o['rd']>0:
            L.append(f"op{i}-ks-break-pt = {bp}")
        L.append(f"op{i}-ks-l-depth = {o['ld']}")
        L.append(f"op{i}-ks-r-depth = {o['rd']}")   # default 30 != DX7 0, always emit
        if o['ld']>0 and o['lc']!=0: L.append(f'op{i}-ks-l-curve = "{KS_CURVE[o["lc"]]}"')
        if o['rd']>0 and o['rc']!=2: L.append(f'op{i}-ks-r-curve = "{KS_CURVE[o["rc"]]}"')
        L.append("")

    # pitch EG — only if it actually moves pitch (any level != 50)
    if any(l!=50 for l in v['peg_l']):
        for k,r in enumerate(v['peg_r'],1): L.append(f"peg-r{k} = {r}")
        for k,l in enumerate(v['peg_l'],1):
            sv_=max(-99,min(99,(l-50)*2)); L.append(f"peg-l{k} = {sv_}")
        # Full-scale swing in semitones (l=±99). DX7 PEG extreme ≈ ±4 octaves;
        # matches the peg-depth param default, emitted explicit for reproducibility.
        L.append("peg-depth = 48.0")
        L.append("")

    # LFO -> lfo2 + matrix routes (vibrato from PMD, tremolo from AMD)
    routes=[]
    pmd,amd,pms = v['lfo_pmd'],v['lfo_amd'],v['lfo_pms']
    # DX7 vibrato -> matrix global-pitch depth. global-pitch 1.0 = +/-24 st, so
    # depth = (pmd/99)*(pitchmodsens[pms]/255) lands typical patches at ~15-30c
    # and the max (pmd99/pms7) at ~+/-2 st.
    vib = (pmd/99.0)*(PITCHMODSENS[pms]/255.0)
    trem_ops=[(i,o['ams']) for i,o in enumerate(v['ops'],1) if o['ams']>0]
    if vib>0.0005 or (amd>0 and trem_ops):
        L.append(f'lfo2-shape = "{LFO_SHAPE.get(v["lfo_wave"],"Sine")}"')
        L.append(f"lfo2-rate = {round(max(0.06,(v['lfo_speed']/99)**2*47),2)}")
        dly = round((v['lfo_delay']/99)**2*4000)
        if dly: L.append(f"lfo2-delay = {float(dly)}")
        L.append("lfo2-fade = 0.0")   # DX7 is delay-then-on, no fade ramp
        L.append("")
        if vib>0.0005:
            routes.append(("lfo2","global-pitch",round(min(1.0,vib),4)))
        if amd>0:
            for i,ams in trem_ops[:6]:
                d=round((amd/99.0)*(ams/3.0)*0.3,4)
                if d>0: routes.append(("lfo2",f"op{i}-level",d))

    # faithful single DX7 voice
    L.append("stack-density = 1")
    L.append("delay-on = false")
    L.append("reverb-on = false")   # raw factory voices
    L.append(f"master-volume = {master_volume(v)}")

    for slot,(s,d,dep) in enumerate(routes[:16]):
        L.append("")
        L.append("[[matrix]]")
        L.append(f"slot = {slot}")
        L.append(f'source = "{s}"')
        L.append(f'dest = "{d}"')
        L.append(f"depth = {dep}")

    # write
    folder=os.path.join(OUT,cat); os.makedirs(folder,exist_ok=True)
    path=os.path.join(folder,disp+".toml"); n=2
    while path in used_paths:
        path=os.path.join(folder,f"{disp} ({n}).toml"); n+=1
    used_paths.add(path)
    open(path,"w").write("\n".join(L)+"\n")
    return cat,disp

KEEP = {  # hand-authored presets — never delete/overwrite
 "Bell/Tubular Bells.toml","Bass/FM Bass.toml","Organ/Drawbar Organ.toml",
 "Keys/Harpsichord.toml","Mallet/Marimba.toml",
 # pre-existing original factory presets (NOT DX7-derived) — must survive regen
 "Brass/Analog Brass.toml","Keys/Mark II E-Piano.toml",
 "Lead/Analytic Saw Supersaw.toml","Lead/Analytic Square.toml",
 "Lead/Saw 2.toml","Lead/Saw 3.toml","Lead/Saw.toml","Lead/Solo Lead.toml",
 "Pad/Soft Ambient Pad.toml","Strings/FM Strings.toml"}

def clean():
    for root,_,files in os.walk(OUT):
        for f in files:
            if not f.endswith(".toml"): continue
            rel=os.path.relpath(os.path.join(root,f),OUT)
            if rel in KEEP: continue
            os.remove(os.path.join(root,f))

def main():
    roms = rom_dir()
    missing = [b for b in BANKS if not os.path.exists(os.path.join(roms, b + ".syx"))]
    if missing:
        sys.exit(
            f"missing ROM dumps in {roms!r}: {', '.join(b + '.syx' for b in missing)}\n"
            f"set $VXN2_DX7_ROMS or drop the .syx files in tools/roms/ "
            f"(see tools/README.md)")
    clean()
    seen={}; order=[]
    for b in BANKS:
        data=open(os.path.join(roms, b + ".syx"),"rb").read()
        body=data[6:6+4096]
        for vi in range(32):
            raw=body[vi*128:(vi+1)*128]
            v=dec.unpack(raw)
            nm=v['name'].strip()
            if nm in SKIP: continue
            key=nm.upper()  # dedup by display name; bank order keeps ROM1/2 originals
            if key in seen: continue
            seen[key]=True; order.append(v)
    used={os.path.join(OUT,k) for k in KEEP}; cats={}
    for v in order:
        cat,disp=emit(v,used)
        cats.setdefault(cat,[]).append(disp)
    total=sum(len(x) for x in cats.values())
    print(f"wrote {total} presets across {len(cats)} categories")
    for c in sorted(cats): print(f"  {c:8s} {len(cats[c]):3d}  {', '.join(cats[c][:6])}{' ...' if len(cats[c])>6 else ''}")

if __name__=="__main__": main()
