"""Regenerates the encrypted fixtures used by crates/cos/tests/encryption.rs.

The fixtures are committed; run this only to change them (output differs on every run: qpdf
picks random IDs and salts for encrypted files). The encryption is done by qpdf (through
pikepdf), so the files are an independent check of cos' decryption. Every file has the same
content, which the tests look for:

* /Info /Title is "Secret title" (an indirect string; in an object stream when one is used)
* the page's content stream (Flate, compressed by qpdf on save) draws "Hello, encrypted world"
* the catalog's /Metadata stream holds the marker VELLORA-METADATA
* the catalog's /Marker is the string "Catalog marker"
* the catalog's /SigTest is a /Type /Sig dictionary whose /Contents ("SIGNATURE-BYTES-0123456789")
  qpdf leaves unencrypted, as the standard requires, while its /Reason ("because") is encrypted

Passwords: the owner password is "owner-pw" in every file; files named *-user-password have the
user password "user-pw", all others the empty user password.

    uv venv .venv && uv pip install --python .venv/Scripts/python.exe pikepdf
    .venv/Scripts/python.exe generate.py          (pikepdf 10.x, qpdf 12.4)
"""

from pathlib import Path

import pikepdf
from pikepdf import Dictionary, Encryption, Name, ObjectStreamMode, Pdf, String

OUT = Path(__file__).parent
OWNER = "owner-pw"
USER = "user-pw"

XMP = (
    b'<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/">'
    b'<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
    b'<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">'
    b"<dc:format>VELLORA-METADATA</dc:format>"
    b"</rdf:Description></rdf:RDF></x:xmpmeta>"
    b'<?xpacket end="w"?>'
)


def build() -> Pdf:
    pdf = Pdf.new()
    content = pdf.make_stream(
        b"BT /F1 24 Tf 72 700 Td (Hello, encrypted world) Tj ET",
    )
    font = pdf.make_indirect(
        Dictionary(Type=Name.Font, Subtype=Name.Type1, BaseFont=Name.Helvetica)
    )
    page = pdf.make_indirect(
        Dictionary(
            Type=Name.Page,
            MediaBox=[0, 0, 612, 792],
            Contents=content,
            Resources=Dictionary(Font=Dictionary(F1=font)),
        )
    )
    pdf.Root.Pages = pdf.make_indirect(
        Dictionary(Type=Name.Pages, Kids=[page], Count=1)
    )
    page.Parent = pdf.Root.Pages
    pdf.Root.Marker = String("Catalog marker")
    pdf.Root.SigTest = pdf.make_indirect(
        Dictionary(
            Type=Name.Sig,
            Filter=Name.Adobe_PPKLite,
            ByteRange=[0, 1, 2, 3],
            Contents=String(b"SIGNATURE-BYTES-0123456789"),
            Reason=String("because"),
        )
    )
    metadata = pdf.make_stream(XMP)
    metadata.stream_dict.Type = Name.Metadata
    metadata.stream_dict.Subtype = Name.XML
    pdf.Root.Metadata = metadata
    pdf.trailer.Info = pdf.make_indirect(Dictionary(Title=String("Secret title")))
    return pdf


# name: (Encryption arguments, user password, object streams). qpdf refuses `metadata=True`
# and AES below R4 (those options do not exist there; metadata is encrypted like everything else),
# and encrypted metadata without AES.
FIXTURES = {
    "r2-rc4-40": (dict(R=2, aes=False, metadata=False), "", False),
    "r3-rc4-128": (dict(R=3, aes=False, metadata=False), "", False),
    "r4-rc4-128": (dict(R=4, aes=False, metadata=False), "", False),
    "r4-aes-128": (dict(R=4, aes=True), "", True),
    "r4-aes-128-no-metadata": (dict(R=4, aes=True, metadata=False), "", False),
    "r6-aes-256": (dict(R=6), "", True),
    "r2-rc4-40-user-password": (dict(R=2, aes=False, metadata=False), USER, False),
    "r3-rc4-128-user-password": (dict(R=3, aes=False, metadata=False), USER, False),
    "r4-aes-128-user-password": (dict(R=4, aes=True), USER, True),
    "r6-aes-256-user-password": (dict(R=6), USER, True),
}


def main() -> None:
    for name, (options, user, objstm) in FIXTURES.items():
        pdf = build()
        pdf.save(
            OUT / f"{name}.pdf",
            encryption=Encryption(user=user, owner=OWNER, **options),
            object_stream_mode=ObjectStreamMode.generate
            if objstm
            else ObjectStreamMode.disable,
        )
        print("wrote", name)


if __name__ == "__main__":
    main()
