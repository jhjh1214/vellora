# Viewing a document

## Moving through the pages

| Action | How |
|---|---|
| Scroll | Mouse wheel, scroll bars, arrow keys, Page Up and Page Down. |
| Next or previous page | View -> Next Page and Previous Page (Ctrl+Down, Ctrl+Up). |
| Go to a page | View -> Go to Page… (Ctrl+G), or click the page text in the status bar. Type a number or the page's label as the document shows it (`iv`, `A-3`). |
| Go back to where you were | View -> Previous View (Alt+Left) and Next View (Alt+Right). Jumps are remembered (outline, Go to Page, thumbnails, links inside the document); scrolling is not. |
| Thumbnails | View -> Page Thumbnails (F4). Click a thumbnail to go to that page. |
| Outline | View -> Outline (Ctrl+Shift+O): the bookmarks of the document, opened one level at a time. |

## Zoom

| Action | How |
|---|---|
| Zoom in and out | View -> Zoom -> Zoom In and Zoom Out (Ctrl++ and Ctrl+-), or hold Ctrl and turn the mouse wheel. |
| Fixed sizes | View -> Zoom: Actual Size (Ctrl+1), Fit Width (Ctrl+2), Fit Page (Ctrl+0), and the presets from 25% to 6400%. |
| Zoom to an area | View -> Zoom to Selection, then drag a rectangle; or hold Z while you drag. Escape cancels. |

The zoom goes from 10% to 6400%. The current zoom is in the status bar.

## Page layout

View -> Page Layout chooses how pages are arranged:

- **Single Page** or **Continuous Scrolling** (one column).
- **Two Pages** or **Two Pages, Continuous** (side by side).
- **Cover Page in Two-Page View** keeps the first page on its own, as the cover of a book.

View -> Rotate Clockwise and Rotate Counterclockwise (Ctrl+] and Ctrl+[) turn the view in steps of a quarter turn. The file is not changed.

## Links

- A link to another place in the document takes you there, and Alt+Left brings you back.
- A link to a web address (`http`, `https`) or an e-mail address (`mailto`) first **asks**, showing the whole address; nothing is opened until you say yes. Addresses with any other scheme are shown and refused.
- Links that would start a program or run a script are shown as links that Vellora does not follow, and say so.
- With the keyboard: View -> Next Link and Previous Link (F8, Shift+F8) move through the links of the page, and Enter or Space follows the one in focus.

## Keyboard reading

View -> Caret Browsing (F7) puts a text cursor in the page: the arrow keys move it, Ctrl+arrows move by word, Home and End go to the ends of a line, and Shift extends a selection. It is meant for people who read without a mouse; see [selecting and copying text](text-selection.md).

Vellora exposes its window to the screen reader of your system. This has been checked in the tests; a check with real screen readers has not been done yet, so tell us if something is not announced.
