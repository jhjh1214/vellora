# Printing a document

Vellora prints to the printers your system knows. The engine draws each page at the printer's own resolution, piece by piece, so a large page at a high resolution does not need a large amount of memory.

## What you can do

| Action | How |
|---|---|
| Print | File → Print (Ctrl+P). |
| Preview | File → Print Preview. It shows the pages as they will be laid out, drawn the same way at screen resolution. |
| Stop a print | **Cancel** in the progress window. The pages not yet drawn are not asked for, and the printer's job is dropped. |

Printing takes two steps. First Vellora asks what to print and how; then your system's own print dialog asks for the printer, the paper and the number of copies.

## Pages

| Choice | Meaning |
|---|---|
| All | Every page. |
| Current page | The page on screen. |
| Pages | A list such as `1-3, 7`. Page **labels** work too, as the document shows them: `iv`, `A-3`, `ii-2`. A label wins over a number: in a document whose front matter is numbered i, ii and whose first chapter starts at 1, `1` is the chapter's first page, not the document's. Each page is printed once, in the order you typed. |
| Odd or even pages only | Counted by page number (the first page is odd). Useful for printing on both sides by hand. |

A range that cannot be read ("11" in a ten-page document, "5-2") is refused and the dialog says which part.

## Page size

| Option | Meaning |
|---|---|
| Fit to page | The page is enlarged or reduced to fill the paper. |
| Actual size | One point of the page is one point on the paper. A page larger than the paper is cut off. |
| Shrink oversized pages | Actual size, except that a page larger than the paper is reduced to fit. |
| Turn pages to match the paper | A landscape page on portrait paper (or the reverse) is turned a quarter turn. |
| Centre on the paper | Otherwise the page starts at the top left. |
| Print in grayscale | Colours are printed as greys. |

Annotations and form fields are drawn the way PDF says they print: a note marked "do not print" stays off the paper, and one marked "print only" appears. Copies, collating and the page order (last page first) follow the printer's dialog; a printer that cannot make copies itself is sent each copy in turn.

## Documents that restrict printing

A PDF can say that it may not be printed, or that it may be printed at low resolution only. Vellora follows that:

- **Printing not allowed:** Vellora tells you and does not print. Opening the document with its owner password, when you have it, lifts the restriction.
- **Low resolution only:** Vellora tells you, and prints at 150 dots per inch however fine your printer is. The page is scaled up to fill the paper, so it looks coarse.

If Vellora cannot read what a document allows, it does not print it.

## No "print to PDF" here

Vellora does not offer to write a PDF as a printer, because writing PDF is the job of its own, carefully tested writer. If you want a PDF of a document, copy the file. If you want a printed-to-PDF copy anyway, choose the PDF printer your system provides.
