# Searching a document

Vellora searches the text of the whole document (it does not recognise text in images, so a scanned page has nothing to find). The search runs in the background, page by page, while you keep reading: the first hits appear at once and a document of a thousand pages is searched in well under a second on a modern machine.

## What you can do

| Action | How |
|---|---|
| Open the find bar | Edit → Find (Ctrl+F). The cursor is in the text field. |
| Search | Type. The search starts when you pause for a moment, or press Enter. |
| Next / previous result | F3 and Shift+F3, Enter and Shift+Enter in the find bar, or the ▼ and ▲ buttons. Both wrap round the document. |
| See all results | The **List** button of the find bar, or View → Search Results (Ctrl+Shift+F). The sidebar opens on its Search tab, with the page and the words around each result; click one to go there. |
| Close | Escape, or the ✕ button. The highlights go away. |

Every result is highlighted yellow on the page, and the current one orange. When you start a search the current result is the first one on or after the page you are on; if there is none after it, the first of the document. The view scrolls to the current result unless it is already well inside the window. Choosing a result in the list counts as a jump, so Alt+Left brings you back; stepping with F3 does not.

## Options

| Option | Meaning |
|---|---|
| Match case | "Fish" does not find "fish". Off by default. |
| Whole words | The result must start and end at a word boundary: "cat" does not find "concat". A text that starts or ends with a symbol, such as "C++", cannot be found with this option, because there is no word boundary next to a symbol. |
| Regular expression | The text is a regular expression in the syntax of Rust's `regex` crate (see its documentation): classes such as `[0-9]`, `\d`, groups, alternatives with `\|`, repeats. **Not** supported: look-around (`(?=…)`) and back-references (`\1`). An expression that does not parse is refused and the find bar says why. Expressions that would take too much memory are refused too. |

Without the regular expression option the text is searched for exactly as typed; `.` and `*` mean themselves.

## How the text is matched

- A line break, and any run of spaces, matches a single space in what you typed, so a phrase that runs from one line to the next is found. The highlight covers each line it spans.
- A hyphen at the end of a line stays where it is: "exam-" at the end of a line and "ple" at the start of the next is not found by "example" (the same is true when copying text).
- Letters are compared with Unicode case folding (so "É" matches "é" unless Match case is on). Accents are **not** ignored: "e" does not find "é".
- Ligatures (fi, fl, …) are searched as their letters.
- Right-to-left and vertical text are searched in the order the text is stored, like copying it; the highlight may be imperfect there (see [text-selection.md](text-selection.md)).

## Limits

- At most 10,000 results are listed; the find bar then says "1 of 10,000+ results". Search for something more specific to see the rest.
- The search text is at most 1,024 bytes.
- Only the first 524,288 characters of a page are searched.
- A page that PDFium cannot read has no text to find; the rest of the document is still searched.
- If the page renderer stops while a search runs, it is restarted and the search starts again from the first page.
