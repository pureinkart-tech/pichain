#!/usr/bin/env python3
"""Convert INVESTOR_DECK.md to a professional PDF using weasyprint."""

import markdown
from weasyprint import HTML
from pathlib import Path

MD_FILE = Path(__file__).parent / "INVESTOR_DECK.md"
OUT_FILE = Path(__file__).parent / "PIChain_Investor_Deck.pdf"

CSS = """
@page {
    size: A4;
    margin: 2cm 2.5cm;
    @bottom-center {
        content: "PIChain — Confidential";
        font-size: 8pt;
        color: #999;
    }
    @bottom-right {
        content: counter(page);
        font-size: 8pt;
        color: #999;
    }
}

@page :first {
    margin-top: 3cm;
}

body {
    font-family: 'Segoe UI', 'Helvetica Neue', Arial, sans-serif;
    font-size: 10.5pt;
    line-height: 1.55;
    color: #1a1a2e;
    max-width: 100%;
}

h1 {
    font-size: 26pt;
    font-weight: 800;
    color: #0d1b2a;
    border-bottom: 3px solid #e63946;
    padding-bottom: 10px;
    margin-top: 30px;
    margin-bottom: 15px;
    page-break-after: avoid;
}

h2 {
    font-size: 16pt;
    font-weight: 700;
    color: #1b263b;
    border-bottom: 2px solid #457b9d;
    padding-bottom: 6px;
    margin-top: 28px;
    margin-bottom: 12px;
    page-break-after: avoid;
}

h3 {
    font-size: 12.5pt;
    font-weight: 600;
    color: #264653;
    margin-top: 18px;
    margin-bottom: 8px;
    page-break-after: avoid;
}

h4 {
    font-size: 11pt;
    font-weight: 600;
    color: #2a9d8f;
    margin-top: 14px;
    margin-bottom: 6px;
}

p {
    margin-bottom: 8px;
}

strong {
    color: #0d1b2a;
}

table {
    width: 100%;
    border-collapse: collapse;
    margin: 12px 0 16px 0;
    font-size: 9.5pt;
    page-break-inside: avoid;
}

thead tr {
    background: linear-gradient(135deg, #1b263b, #264653);
    color: white;
}

th {
    padding: 8px 10px;
    text-align: left;
    font-weight: 600;
    border: 1px solid #1b263b;
}

td {
    padding: 6px 10px;
    border: 1px solid #ddd;
}

tbody tr:nth-child(even) {
    background-color: #f8f9fa;
}

tbody tr:hover {
    background-color: #e8f4f8;
}

pre {
    background: #0d1b2a;
    color: #a8dadc;
    padding: 14px 16px;
    border-radius: 6px;
    font-size: 8.5pt;
    line-height: 1.4;
    overflow-x: auto;
    white-space: pre-wrap;
    word-wrap: break-word;
    margin: 10px 0 14px 0;
    page-break-inside: avoid;
}

code {
    background: #e8f4f8;
    color: #264653;
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 9.5pt;
}

pre code {
    background: transparent;
    color: #a8dadc;
    padding: 0;
}

ul, ol {
    padding-left: 20px;
    margin-bottom: 10px;
}

li {
    margin-bottom: 4px;
}

hr {
    border: none;
    border-top: 1px solid #ddd;
    margin: 24px 0;
}

blockquote {
    border-left: 4px solid #e63946;
    margin: 12px 0;
    padding: 8px 16px;
    background: #fff3f3;
    color: #333;
}

/* Title page styling */
h1:first-of-type {
    font-size: 32pt;
    text-align: center;
    border-bottom: 4px solid #e63946;
    margin-top: 60px;
    padding-bottom: 15px;
}

h1:first-of-type + h3 {
    text-align: center;
    color: #457b9d;
    font-size: 14pt;
    font-weight: 400;
    margin-bottom: 4px;
}

h1:first-of-type + h3 + h3 {
    text-align: center;
    color: #457b9d;
    font-size: 13pt;
    font-weight: 400;
    font-style: italic;
    margin-bottom: 30px;
}
"""

def main():
    md_text = MD_FILE.read_text(encoding="utf-8")

    # Convert markdown to HTML
    html_body = markdown.markdown(
        md_text,
        extensions=["tables", "fenced_code", "codehilite", "toc"],
        extension_configs={
            "codehilite": {"css_class": "highlight", "guess_lang": False}
        },
    )

    full_html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<style>{CSS}</style>
</head>
<body>
{html_body}
</body>
</html>"""

    HTML(string=full_html).write_pdf(str(OUT_FILE))
    print(f"PDF generated: {OUT_FILE}")
    print(f"Size: {OUT_FILE.stat().st_size / 1024:.0f} KB")

if __name__ == "__main__":
    main()
