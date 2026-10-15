#!/usr/bin/env python3
"""Original synthetic image fixtures for decoder and cell qualification."""
import io
from pathlib import Path
import struct
import zipfile
import zlib
ROOT = Path(__file__).resolve().parents[1] / "fixtures/images"
ROOT.mkdir(parents=True, exist_ok=True)
# RGB red, green / blue, transparent white (hidden RGB deliberately retained).
pixels = [(255,0,0,255),(0,255,0,255),(0,0,255,255),(255,255,255,0)]
def png_chunk(kind, data):
    return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data))
png = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR",struct.pack(">IIBBBBB",2,2,8,6,0,0,0))
png += png_chunk(b"IDAT",zlib.compress(b"".join(b"\0"+bytes(sum((list(p) for p in pixels[y*2:y*2+2]),[])) for y in range(2)))) + png_chunk(b"IEND",b"")
(ROOT/"checker.png").write_bytes(png)
# 24-bit bottom-up BMP with four-byte padded rows.
data = b"".join(b"".join(bytes((b,g,r)) for r,g,b,a in pixels[y*2:y*2+2])+b"\0\0" for y in [1,0])
(ROOT/"checker.bmp").write_bytes(b"BM"+struct.pack("<IHHI",54+len(data),0,0,54)+struct.pack("<IiiHHIIiiII",40,2,2,1,24,0,len(data),0,0,0,0)+data)
# Uncompressed true-color TGA, top origin, eight alpha bits.
header=bytearray(18); header[2]=2; header[12:16]=struct.pack("<HH",2,2);header[16:18]=bytes([32,0x28])
(ROOT/"checker.tga").write_bytes(header+b"".join(bytes((b,g,r,a)) for r,g,b,a in pixels))
# One-frame 1x1 GIF with a red palette entry.
(ROOT/"red.gif").write_bytes(bytes.fromhex("47494638396101000100800000ff00000000002c00000000010001000002024401003b"))
# Planar RGB/RGBA PSD composite, empty metadata/layers, raw and PackBits variants.
planes=[bytes(p[channel] for p in pixels) for channel in range(4)]
psd_header=b"8BPS"+struct.pack(">H",1)+b"\0"*6+struct.pack(">HIIHH",4,2,2,8,3)+b"\0"*12
(ROOT/"checker.psd").write_bytes(psd_header+struct.pack(">H",0)+b"".join(planes))
rows=[plane[y*2:y*2+2] for plane in planes for y in range(2)]
(ROOT/"checker-rle.psd").write_bytes(psd_header+struct.pack(">H",1)+b"".join(struct.pack(">H",3) for row in rows)+b"".join(bytes([1])+row for row in rows))
# DXT1/3/4/5 DDS headers and a solid-red 4x4 color block.
color=struct.pack("<HHI",0xf800,0,0)
for fourcc in [b"DXT1",b"DXT3",b"DXT4",b"DXT5"]:
    block = color if fourcc==b"DXT1" else (b"\xff"*8+color if fourcc==b"DXT3" else bytes([255,0])+b"\0"*6+color)
    header=bytearray(124)
    struct.pack_into("<IIIIIII",header,0,124,0x81007,4,4,len(block),0,0)
    struct.pack_into("<II4sIIIII",header,72,32,4,fourcc,0,0,0,0,0)
    struct.pack_into("<I",header,104,0x1000)
    (ROOT/(fourcc.decode().lower()+".dds")).write_bytes(b"DDS "+header+block)
archive=io.BytesIO()
with zipfile.ZipFile(archive,"w",compression=zipfile.ZIP_DEFLATED) as z:
    for name,content in [("mergedimage.png",png),("maindoc.xml",b'<DOC><IMAGE width="2" height="2"/></DOC>'),("document/",b"")]:
        item=zipfile.ZipInfo(name,date_time=(2020,1,1,0,0,0));item.compress_type=zipfile.ZIP_DEFLATED
        z.writestr(item,content)
(ROOT/"checker.kra").write_bytes(archive.getvalue())
(ROOT/"checker.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><path fill="red" d="M0 0h1v1H0z"/><path fill="lime" d="M1 0h1v1H1z"/><path fill="blue" d="M0 1h1v1H0z"/></svg>\n')
print("Wrote 12 original image fixtures")
