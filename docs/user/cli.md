# The `vellora` command line

```
vellora <COMMAND> [OPTIONS]
vellora --help | --version
```

**Exit codes** (every command):

| Code | Meaning |
|---|---|
| 0 | The command did what was asked. |
| 1 | The operation failed: the file cannot be read, is not a PDF that Vellora can open, or the password is wrong. A message starting with `vellora: ` is written to stderr and nothing to stdout. |
| 2 | Usage error: unknown command, missing or unknown argument. |

Commands never run or open anything found inside a document: no JavaScript, no launch actions, no attachments, no network access.

## `vellora inspect`

```
vellora inspect <FILE> [--json] [--password <PASSWORD>]
```

Reads the file and reports what it contains. The whole file is read into memory.

| Option | Meaning |
|---|---|
| `--json` | Print the report as JSON (schema below) instead of a table. |
| `--password <PASSWORD>` | Password for an encrypted file, user or owner. It is tried as UTF-8 and, if it fits in Latin-1, as Latin-1 bytes; no Unicode normalisation is applied. A wrong password is exit code 1, even for an encrypted file that opens with the empty password; an unencrypted file ignores the option. The password is visible in the process list. |

An encrypted file that needs a password and was not given one still gets a report (exit code 0): the encryption is described, `locked` is `true`, and `pages` and `features` are `null` because nothing can be read. A note about `--password` is written to stderr.

### Table output

```
File          report.pdf
Size          607 bytes
PDF version   1.7
Pages         1
Objects       4
Revisions     1
Repaired      no
Encryption    none

Features
  JavaScript on open              yes
  ...
```

The table is for people; its layout may change. Scripts should use `--json`.

### JSON schema

Field names and meanings are stable within a major version. New fields may be added, so readers must ignore fields they do not know.

| Field | Type | Meaning |
|---|---|---|
| `file_size` | integer | Size of the file in bytes. |
| `version` | string or `null` | Version in the `%PDF-M.m` header, e.g. `"1.7"`; `null` if the header has none. |
| `objects` | integer | Objects the cross-reference lists as in use, compressed ones included. Not checked against the file. |
| `revisions` | integer | Revisions in the file's own incremental history; 1 for a file never updated, and for one whose cross-reference had to be rebuilt. |
| `repaired` | boolean | The file had to be repaired to be read. |
| `repair_reasons` | array of strings | One line per reason; empty when `repaired` is `false`. Free text, for people. |
| `encryption` | object or `null` | `null` for an unencrypted document. See below. |
| `locked` | boolean | Encrypted and no password was accepted. |
| `pages` | integer or `null` | Pages found by walking the page tree (`/Count` is not trusted); `null` when `locked`. |
| `features` | object or `null` | See below; `null` when `locked`. |
| `problems` | array of strings | Things that went wrong while reading, so some numbers may be too low (at most 16). Free text. |

`encryption` (Standard Security Handler):

| Field | Type | Meaning |
|---|---|---|
| `handler` | string | Always `"Standard"`; other handlers are not supported and make the command fail. |
| `version` | integer | `/V`, the algorithm version. |
| `revision` | integer | `/R`, 2 to 6. |
| `key_bits` | integer | Length of the file encryption key in bits. |
| `permissions` | integer | `/P` as a signed 32-bit integer. |
| `allowed` | object | `permissions` decoded into booleans: `print`, `modify`, `copy`, `annotate`, `fill_forms`, `accessibility`, `assemble`, `print_high_quality`. These bind a conforming reader; they are not a security boundary. |
| `stream_method` | string | `"identity"`, `"rc4"`, `"aes-128"` or `"aes-256"`. |
| `string_method` | string | As `stream_method`, for strings. |
| `encrypt_metadata` | boolean | Whether the metadata stream is encrypted. |
| `unlocked_with` | string or `null` | `"user"` (the empty password counts) or `"owner"`; `null` when locked. |

`features` — each flag is `true` when the feature was found in the places the scan looks:
the catalog, every page and its annotations, the form fields, and the bookmarks.
Actions are followed along `/Next` chains. Only dictionaries are read; scripts are never decoded.

| Field | Meaning |
|---|---|
| `complete` | The scan read everything. `false` means it hit its work limit or could not read some objects (see `problems`), so a `false` flag below may be a miss. |
| `open_action_javascript` | The catalog's `/OpenAction` is, or chains to, a JavaScript action: it runs when the file is opened. |
| `javascript_actions` | A JavaScript action anywhere in the scanned places. |
| `names_javascript` | The catalog has `/Names` `/JavaScript` (document-level scripts). |
| `additional_actions` | An `/AA` dictionary on the catalog, a page, an annotation or a form field. |
| `launch_actions` | A `Launch` action. |
| `uri_actions` | A `URI` action. |
| `submit_form_actions` | A `SubmitForm` action. |
| `goto_remote_actions` | A `GoToR` action (opens another PDF file). |
| `embedded_files` | `/Names` `/EmbeddedFiles`, or a file attachment annotation. |
| `acroform` | The catalog has an `/AcroForm`. |
| `xfa` | The `/AcroForm` has `/XFA`. |
| `optional_content` | The catalog has `/OCProperties` (layers). |
| `signature_fields` | A field with type `/Sig`, including one that inherits the type from its parent. Signatures are listed, not verified. |

Example (`vellora inspect --json`, abbreviated):

```json
{
  "file_size": 329,
  "version": "1.7",
  "objects": 3,
  "revisions": 1,
  "repaired": false,
  "repair_reasons": [],
  "encryption": null,
  "locked": false,
  "pages": 1,
  "features": { "complete": true, "open_action_javascript": false, "...": false },
  "problems": []
}
```
