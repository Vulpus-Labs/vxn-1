#!/usr/bin/env python3
"""DX7 sysex (.syx) voice decoder.

Unpacks the 128-byte packed-voice format from a 32-voice DX7 cartridge dump
into plain dicts the converter ([`dx7_to_vxn2.py`]) consumes. Standalone CLI:

    python3 dx7decode.py rom1a.syx            # list the 32 voice names
    python3 dx7decode.py rom1a.syx SAX BELL   # dump matching voices in full

Note: DX7 factory ROM dumps are Yamaha data and are NOT committed to this repo;
supply your own `.syx` files (see tools/README.md).
"""
import sys, struct


def load_voices(path):
    data = open(path, 'rb').read()
    assert data[0] == 0xF0 and data[1] == 0x43, "not a syx"
    # header 6 bytes, then 4096 data
    body = data[6:6 + 4096]
    voices = []
    for v in range(32):
        b = body[v * 128:(v + 1) * 128]
        voices.append(unpack(b))
    return voices


def unpack(b):
    ops = []
    # operators stored op6..op1, 17 packed bytes each (but packed=17)
    for i in range(6):
        o = b[i * 17:(i + 1) * 17]
        eg_r = list(o[0:4]); eg_l = list(o[4:8])
        brk = o[8]; ld = o[9]; rd = o[10]
        lc = o[11] & 0x03; rc = (o[11] >> 2) & 0x03
        detune = (o[12] >> 3) & 0x0f; rs = o[12] & 0x07
        kvs = (o[13] >> 2) & 0x07; ams = o[13] & 0x03
        out = o[14]
        mode = o[15] & 0x01; coarse = (o[15] >> 1) & 0x1f
        fine = o[16]
        ops.append(dict(eg_r=eg_r, eg_l=eg_l, brk=brk, ld=ld, rd=rd, lc=lc, rc=rc,
            detune=detune, rs=rs, kvs=kvs, ams=ams, out=out, mode=mode, coarse=coarse, fine=fine))
    # ops list is op6..op1 -> reverse to op1..op6
    ops = ops[::-1]
    g = {}
    g['peg_r'] = list(b[102:106]); g['peg_l'] = list(b[106:110])
    g['algo'] = b[110] + 1  # 1-based
    g['sync'] = (b[111] >> 3) & 1; g['feedback'] = b[111] & 0x07
    g['lfo_speed'] = b[112]; g['lfo_delay'] = b[113]
    g['lfo_pmd'] = b[114]; g['lfo_amd'] = b[115]
    g['lfo_pms'] = (b[116] >> 4) & 0x07; g['lfo_wave'] = (b[116] >> 1) & 0x07; g['lfo_sync'] = b[116] & 1
    g['transpose'] = b[117]
    g['name'] = bytes(b[118:128]).decode('ascii', 'replace')
    return dict(ops=ops, **g)


WAVE = ['Triangle', 'SawDown', 'SawUp', 'Square', 'Sine', 'S&H']


def coarse_ratio(coarse, fine):
    base = 0.5 if coarse == 0 else float(coarse)
    return base * (1 + fine / 100.0)


def show(v):
    print(f"=== {v['name'].strip()!r}  algo={v['algo']} fb={v['feedback']} sync={v['sync']}")
    print(f"    PEG rate={v['peg_r']} level={v['peg_l']} transpose={v['transpose']-24:+d}st")
    lw = WAVE[v['lfo_wave']] if v['lfo_wave'] < 6 else v['lfo_wave']
    print(f"    LFO spd={v['lfo_speed']} dly={v['lfo_delay']} pmd={v['lfo_pmd']} amd={v['lfo_amd']} pms={v['lfo_pms']} wave={lw} sync={v['lfo_sync']}")
    for i, o in enumerate(v['ops'], 1):
        r = coarse_ratio(o['coarse'], o['fine'])
        modestr = 'FIXED' if o['mode'] else f"ratio={r:.2f}(c{o['coarse']}/f{o['fine']})"
        print(f"  op{i}: out={o['out']:2d} {modestr} det={o['detune']-7:+d} "
              f"EGr={o['eg_r']} EGl={o['eg_l']} kvs={o['kvs']} ams={o['ams']} "
              f"rs={o['rs']} brk={o['brk']} ld={o['ld']} rd={o['rd']} lc={o['lc']} rc={o['rc']}")


if __name__ == '__main__':
    path = sys.argv[1]
    voices = load_voices(path)
    if len(sys.argv) > 2:
        # filter by name substrings
        wants = sys.argv[2:]
        for v in voices:
            nm = v['name'].strip().upper()
            if any(w.upper() in nm for w in wants):
                show(v)
    else:
        for i, v in enumerate(voices):
            print(f"{i:2d} {v['name'].strip()!r}")
