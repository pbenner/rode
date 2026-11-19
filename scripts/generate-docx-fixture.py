#!/usr/bin/env python3
"""Build a repository-owned three-page Word fixture without Office or third-party tools.

The package exercises page breaks, styled text, a table, numbering, an embedded
image, headers/footers and a footnote. Fixed ZIP metadata makes it reproducible.
"""

import argparse
import importlib.util
import io
import zipfile
from pathlib import Path

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL = "http://schemas.openxmlformats.org/package/2006/relationships"
TYPES = "http://schemas.openxmlformats.org/package/2006/content-types"


def paragraph(text, *, color="202020", size=24, extra="", props=""):
    """Emit a paragraph with explicit Word run formatting for stable visual assertions."""
    return f'<w:p><w:pPr>{props}</w:pPr><w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size}"/></w:rPr><w:t>{text}</w:t></w:r>{extra}</w:p>'


def document():
    """Return a complete DOCX package, including its image and related stories."""
    image_spec = importlib.util.spec_from_file_location(
        "pptx_fixture", Path(__file__).with_name("generate-pptx-fixture.py")
    )
    images = importlib.util.module_from_spec(image_spec)
    image_spec.loader.exec_module(images)
    heading = paragraph(
        "RODE Word preview", color="2048E8", size=48, props='<w:jc w:val="center"/>'
    )
    styled = "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold text. </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>Italic text.</w:t></w:r></w:p>"
    picture = '<w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Green square"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="Green square"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="image"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>'
    table = '<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblBorders><w:top w:val="single" w:sz="8"/><w:left w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="8"/><w:right w:val="single" w:sz="8"/><w:insideH w:val="single" w:sz="8"/><w:insideV w:val="single" w:sz="8"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="3600"/><w:gridCol w:w="3600"/></w:tblGrid>'
    for cells in (("Topic", "Result"), ("Native rendering", "No Office installation")):
        table += (
            "<w:tr>"
            + "".join(
                f'<w:tc><w:tcPr><w:tcW w:w="3600" w:type="dxa"/></w:tcPr>{paragraph(cell)}</w:tc>'
                for cell in cells
            )
            + "</w:tr>"
        )
    table += "</w:tbl>"
    page_break = '<w:p><w:r><w:br w:type="page"/></w:r></w:p>'
    body = heading + styled + picture + table + page_break
    body += paragraph("Second Word page", color="E82525", size=40)
    body += paragraph(
        "Numbered item", props='<w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>'
    )
    body += paragraph(
        "A paragraph with a footnote", extra='<w:r><w:footnoteReference w:id="1"/></w:r>'
    )
    body += page_break + paragraph("Final Word page", color="20BD55", size=40)
    body += paragraph("The original package remains read-only.")
    body += '<w:sectPr><w:headerReference w:type="default" r:id="header"/><w:footerReference w:type="default" r:id="footer"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720"/></w:sectPr>'
    parts = {
        "word/document.xml": f'<w:document xmlns:w="{W}" xmlns:r="{R}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>',
        "word/styles.xml": f'<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/><w:sz w:val="24"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>',
        "word/numbering.xml": f'<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="1"/></w:num></w:numbering>',
        "word/header1.xml": f'<w:hdr xmlns:w="{W}">{paragraph("RODE header", color="777777", size=20)}</w:hdr>',
        "word/footer1.xml": f'<w:ftr xmlns:w="{W}"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>RODE footer — page </w:t></w:r><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p></w:ftr>',
        "word/footnotes.xml": f'<w:footnotes xmlns:w="{W}"><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1">{paragraph("Word footnote content", size=20)}</w:footnote></w:footnotes>',
        "word/media/green.png": images.image(),
    }
    relationships = [
        ("styles", "styles", "styles.xml"),
        ("numbering", "numbering", "numbering.xml"),
        ("header", "header", "header1.xml"),
        ("footer", "footer", "footer1.xml"),
        ("footnotes", "footnotes", "footnotes.xml"),
        ("image", "image", "media/green.png"),
    ]
    parts["word/_rels/document.xml.rels"] = (
        f'<Relationships xmlns="{REL}">'
        + "".join(
            f'<Relationship Id="{key}" Type="{R}/{kind}" Target="{target}"/>'
            for key, kind, target in relationships
        )
        + "</Relationships>"
    )
    parts["_rels/.rels"] = (
        f'<Relationships xmlns="{REL}"><Relationship Id="document" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>'
    )
    kinds = {
        "document": "document.main",
        "styles": "styles",
        "numbering": "numbering",
        "header1": "header",
        "footer1": "footer",
        "footnotes": "footnotes",
    }
    parts["[Content_Types].xml"] = (
        f'<Types xmlns="{TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/>'
        + "".join(
            f'<Override PartName="/word/{name}.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml"/>'
            for name, kind in kinds.items()
        )
        + "</Types>"
    )
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for name, contents in sorted(parts.items()):
            info = zipfile.ZipInfo(name, (2000, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, contents.encode() if isinstance(contents, str) else contents)
    return output.getvalue()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_bytes(document())
