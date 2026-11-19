#!/usr/bin/env python3
"""Generate the repository-owned PPTX regression fixture using only Python's standard library.

The slides contain text, geometry and embedded images; slide three deliberately
contains an invalid image so missing-content diagnostics can be checked. No Office
software or downloaded assets are involved. ZIP timestamps are fixed for reproducibility.
"""

import argparse
import io
import struct
import zipfile
import zlib
from pathlib import Path

P = "http://schemas.openxmlformats.org/presentationml/2006/main"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL = "http://schemas.openxmlformats.org/package/2006/relationships"


def image():
    """Return an opaque green PNG generated from raw pixels."""

    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload))
        )

    raw = (b"\x00" + bytes((25, 190, 80, 255)) * 32) * 32
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 32, 32, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


def presentation(width=9144000, height=5143500):
    """Build three independently navigable slides, with no external relationships."""
    overrides = '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
    overrides += "".join(
        f'<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>'
        for i in range(1, 4)
    )
    parts = {
        "[Content_Types].xml": f'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>{overrides}</Types>',
        "_rels/.rels": f'<Relationships xmlns="{REL}"><Relationship Id="doc" Type="{R}/officeDocument" Target="ppt/presentation.xml"/></Relationships>',
        "ppt/presentation.xml": f'<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldIdLst>'
        + "".join(f'<p:sldId id="{255 + i}" r:id="s{i}"/>' for i in range(1, 4))
        + f'</p:sldIdLst><p:sldSz cx="{width}" cy="{height}"/></p:presentation>',
        "ppt/_rels/presentation.xml.rels": f'<Relationships xmlns="{REL}">'
        + "".join(
            f'<Relationship Id="s{i}" Type="{R}/slide" Target="slides/slide{i}.xml"/>'
            for i in range(1, 4)
        )
        + "</Relationships>",
        "ppt/media/green.png": image(),
        "ppt/media/broken.png": b"deliberately invalid image",
    }
    for i, color in enumerate(("E82525", "2048E8", "D9B92B"), 1):
        text = f'<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="228600"/><a:ext cx="8000000" cy="700000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="3200"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Inter"/></a:rPr><a:t>RODE — PowerPoint slide {i}</a:t></a:r></a:p></p:txBody></p:sp>'
        picture = (
            '<p:pic><p:nvPicPr><p:cNvPr id="3" name="Embedded image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="image"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="6705600" y="3200400"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>'
            if i > 1
            else ""
        )
        parts[f"ppt/slides/slide{i}.xml"] = (
            f'<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{text}{picture}</p:spTree></p:cSld></p:sld>'
        )
        if i > 1:
            name = "green" if i == 2 else "broken"
            parts[f"ppt/slides/_rels/slide{i}.xml.rels"] = (
                f'<Relationships xmlns="{REL}"><Relationship Id="image" Type="{R}/image" Target="../media/{name}.png"/></Relationships>'
            )
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for name, data in sorted(parts.items()):
            info = zipfile.ZipInfo(name, (2000, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data.encode() if isinstance(data, str) else data)
    return output.getvalue()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--width", type=int, default=9144000, help="Slide width in EMU")
    parser.add_argument("--height", type=int, default=5143500, help="Slide height in EMU")
    arguments = parser.parse_args()
    arguments.output.write_bytes(presentation(arguments.width, arguments.height))
