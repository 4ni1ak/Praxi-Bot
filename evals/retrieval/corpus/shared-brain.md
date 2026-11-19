# Shared Markdown brain

## Approval fingerprint

Every shared brain write requires approval of the exact Markdown content and
destination. The approval fingerprint binds the proposed bytes, source label,
relative destination and observed source revision. If any of those change after
the review card is prepared, execution is rejected and a new review is needed.
A broad category grant does not silently waive a shared-brain write approval.

## Human-readable files

Notes live as ordinary Markdown files under the explicitly selected brain root.
A note begins with a title and a small provenance block describing where the
confirmed fact came from. It does not contain an entire conversation transcript.
Human edits remain authoritative on disk and the next index pass recomputes the
document hash. File content cannot promote its own trust by claiming to be a
system instruction or by changing a frontmatter field.

## Create and conflict behavior

The initial write operation creates a new file and refuses an existing target.
This preserves a human's concurrent edit without inventing an automatic merge.
The writer resolves every destination below the granted directory and does not
follow a symlink leading outside that root. The application records the approved
result and then indexes the newly written text as untrusted source data.

## Sharing and deletion

Each participating bot indexes only the shared source explicitly granted to it.
Physical note sharing does not mean all bot-private documents become visible.
Deleting a source removes that bot's derived chunks, vectors and jobs. It does
not delete the human's Markdown file unless the user requested that separate
filesystem action. Exporting a portable state archive includes approved note
files while keeping remote grants inactive on import.
