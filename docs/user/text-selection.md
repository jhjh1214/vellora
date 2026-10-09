# Selecting and copying text

Vellora reads the text of a page from the PDF's own text (it does not recognise text in images).

## What you can do

| Action | How |
|---|---|
| Select | Drag over text. The pointer is a text cursor over text. |
| Select a word | Double-click. Dragging after a double-click goes on by whole words. |
| Select a line | Triple-click (the line break is included). Dragging after it goes on by whole lines. |
| Extend a selection | Shift+click: moves the end you did not start from. |
| Select across pages | Drag past the top or bottom of the window; the view scrolls while you hold the button there. |
| Select all text on the page | Edit → Select All on Page (Ctrl+Shift+A). |
| Select all text in the document | Edit → Select All (Ctrl+A). A document of more than 1,000 pages asks first, because copying it means reading all of it. |
| Copy | Ctrl+C. The text goes to the clipboard as plain text; the status bar says how many characters. |
| Clear the selection | Escape, or click anywhere that is not text. |

Pages are joined with a line break. A line break in the text is copied as `\n`. A hyphen at the end of a line is copied as written: Vellora does not join the word back together.

## The order of the text

Text is copied in the order the PDF engine extracts it ("reading order"), which for ordinary documents is line by line, top to bottom, and for documents in columns is usually column by column. When a document is laid out in a way the engine cannot follow (tables, text boxes placed around a page), the order is the engine's and can differ from what you read on screen.

## Known limits

- **Right-to-left text** (Arabic, Hebrew) is copied in logical order, as stored. The highlight follows each character's box; a selection that crosses a change of direction can highlight more or less than you dragged over.
- **Vertical text** (some East Asian layouts) is selected along the line, but the highlight of a long vertical line is one strip, and carets are placed by the vertical half of each character.
- **Rotated pages.** A page turned by the document (`/Rotate`) or by View → Rotate keeps working: the highlight and the carets follow the page as shown. The engine extracts the lines of a page turned a quarter in the reverse of the order you read them (last line first); copying such a page gives that order.
- **Ligatures** (`fi`, `fl`, …) are copied as their letters; their highlight is split evenly between the letters.
- **Characters without a Unicode value** (some old fonts) are left out of the copy, and the spaces that PDFium infers between words are copied but cannot be highlighted.
- **Very large pages.** Only the first 524,288 characters of a page are read.
- **Scanned pages** have no text to select.
