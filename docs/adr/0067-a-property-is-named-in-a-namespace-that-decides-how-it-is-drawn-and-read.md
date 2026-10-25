# A property is named in a namespace that decides how it is drawn and read

> Proposed. Extends the vocabulary of [ADR-0047](0047-the-analysis-graph-is-the-case-s-model.md)
> with a catalogue of property names, built the way the type namespaces are, and uses the
> reference grammar of [ADR-0066](0066-the-case-is-addressed-by-references.md) for the values
> that point at something. Nothing here is built. The GUI's graph panel is touched in two
> places only, the properties of a card and the *Details* pane of the inspector; the rest of
> that panel stands.

An element carries `props`, a JSON object of whatever the writer chose to record. The agent
uses it a great deal, and it is useful: a size, a path, a command line, a source line that
does not belong in the text. Two things make it worse than it should be.

- **The names drift.** `graph_write` describes `props` with one example
  (`search_negative: {query, window_from, window_to}`), so every agent and every session
  invents its own: `size`, `size_bytes`, `file_size`; `first_seen`, `first_observed`. A
  filter, a comparison or a person scanning a column cannot rely on any of them.
- **The values are drawn as text.** The GUI turns every value into a string; a number,
  a list or an object becomes its JSON. A card shows them in a monospace column, wrapped,
  and the inspector as label and value rows. A size, an ISO time, a path and a reference to
  another element all look the same, and none of them can be clicked.

The vocabulary already has a way to say what a family of things is and how it is drawn: a
dotted **namespace** gives its types a common meaning and a look, and a type the agent makes
up under it inherits both ([vocab.rs](../../crates/reactor-db/src/graph/vocab.rs)). This
applies the same to the names of properties. The namespace of a property name is what it is
*drawn as and read as*; what follows it is the writer's.

## Decisions

**A property name is dot-namespaced, and its namespace is a shape.** `time.first_seen`,
`size`, `size.sent`, `code.command_line`, `ref.see`, `flag.hidden`. A name is lowercase
`snake_case` segments joined by dots. The first segment is the namespace; a name that is
only a namespace (`size`, `duration`) is valid and is that shape with nothing
more said. The namespaces are the same for every element: the catalogue does not look at the
element's kind or type, so `size` is a size on a file, a blob, a transfer and a log.

**The namespaces are a closed set, one renderer each; the names under them are open.** A
namespace row is `(prefix, shape, definition, icon, colour)`. The shape is one of the
renderers the GUI has; the person or the agent can add a prefix that points at one of them,
but not a shape. Under a namespace, anything goes: `size.payload`, `time.last_logon`,
`code.yara`, `category.threat_group`. A name inherits the shape of its longest known prefix, the
way `Vocabulary::look` finds a type's icon, so a sub-namespace could say something more
specific (`code.sql`) without the agent having to know it.

| Namespace | The value is | Drawn as | Read as |
|---|---|---|---|
| `text` | prose or a short label | the string, wrapped | text; searched and sorted as text |
| `time` | an ISO 8601 instant | the short form of the case's clock, the full value on hover | an instant; compared and sorted as one |
| `duration` | milliseconds | `4.2 s`, the exact count on hover | a length of time |
| `size` | bytes | `180 KiB`, the exact count on hover | a number of bytes |
| `number` | a number, a count included | as written, no grouping, no unit | a number; compared and sorted numerically |
| `id` | an opaque identifier (a pid, a SID, a digest) | monospace, copy; a long one shortened from the middle | text, compared without case; never summed |
| `flag` | true or false | a yes/no chip | a boolean |
| `ref` | a source reference of ADR-0066: a place in the case's own evidence | a chip with the kind's icon and a label; a click opens it in the window | a reference |
| `uri` | a technical location: a URL or a path; a value with no scheme (`/etc/hosts`, `C:\Users`) is a file path | monospace, shortened (host and path for a URL, from the middle for a path), copy; a click opens it by its scheme: `http` and `https` in the browser, `file` and plain paths in the explorer when the file is registered | a URI; compared as text |
| `code` | source, a command or a record | a monospace block, as many lines as it has | text, kept as written |
| `category` | a short classification (a severity, a protocol, an ATT&CK id) | a chip, coloured as the same text is as a node tag; it draws no group around anything | text, compared without case |

`ref` and `uri` are kept apart on purpose. A `ref` points at the case's own evidence (a graph element, a session entry, a registered file) and is the source of a claim; a `uri` is a technical value the evidence mentioned (a download URL, a working directory) and may point at nothing the case holds.

**One value or several, in the value's own form.** Any shape takes a single JSON value or
an array of them; an array is drawn as one item per row, or as a row of chips for `category` and
`ref`. There is no list shape. A value that does not suit its namespace (a string under
`size`, an object under `time`) is stored as written and drawn as text with the shape's
mark struck through, and the write says so; it is never refused. A JSON object is drawn as a
collapsed key and value tree whatever its namespace.

**What the namespace gives, the name does not need to say.** The row's label is the part
after the namespace, humanised: `time.first_seen` reads *First seen* with the time icon,
`size` reads *Size*. There is no enum shape and no list of allowed values to define ahead:
what was a closed choice (a severity, a protocol, a direction) is a `category.<name>`, a chip
with whatever text the writer put, coloured by the colour the case has given that text as a node
tag (`tag_look`), and neutral where it has none. A `category` is not a node tag: a node tag is
on the element, draws the outline around every element that has it, and clusters them; a
category only classifies the element it is on, and draws no group. Where a classification should
cluster, the writer tags the element.

**The agent is told the namespaces and a few names, and is free to make the rest.** The prompt's
graph block says in one line that a property is named `<namespace>.<name>`, lists the
namespaces with a word each, and points at `graph_query` with `what: vocabulary` and
`field` for the names ([ADR-0056](0056-a-tool-says-what-it-does-and-the-rules-are-one-line-each.md)).
`graph_write` and `graph_annotate` describe `props` as "named `<namespace>.<name>`", and drop
the one example. A name with a known namespace is accepted silently, known or new: that is
the use that is wanted. The seed below is small and is a start, not a list to stay within.

**A name with no known namespace is text, and the write says so once.** The result of the
write carries a notice: `size_in_bytes has no namespace; known: text, time, size, ...; it is
drawn as text`. It is stored as written and drawn as `text`. Nothing is guessed from the
value: what the writer wants drawn as a time or a reference, it names in that namespace.

**An alias folds a spelling to a known name when a property is written.** Seeded names carry
the spellings that come to mind (`size_bytes`, `file_size` to `size`; `cmdline`, `command` to
`code.command_line`). The folding happens in the write path, so what is stored, hashed,
exported and diffed is the known name, and a version that differs only in an alias is not a
new version. The namespace makes a spelling *drawn* right; the alias makes it *found* with the
others. A name cannot be both an alias and a known name.

**A field is a detail of an element, never a thing that has an identity of its own.** An
IP address, a host, an account, a process name, a file path and a file's digest are
entities and have types (`net.ipv4`, `host.name`, `account.user`, `process.name`,
`file.path`, `file.hash.sha256`). When a fact is one of those, it is an element linked by a
role (`src`, `actor`, `target`), so that it can be found, merged and contradicted. A property
is what is *said about* an element and has no other home: how big, when first seen, the
command line, the exit code. So defining a name or a prefix refuses one that is a type, a type
alias, or the first segment of a type (`file`, `net`, `host`, `account`, `process`,
`registry`, `analysis`); the namespaces above are chosen not to be any of them. (The `host`, `pid` and `start`
that `process.instance` uses as key fields are existing identity keys and are not touched.)

**The namespace says how a value is compared.** `graph_query` with `what: find` takes
`prop` as `name`, `name=value`, `name>value` or `name<value`, and a namespace prefix such as
`time.` matches every name under it. Time is compared as an instant, size as bytes
(`size>1MiB` is understood), number as a number, the rest as text. The names are
folded through the aliases first, so a search for `size_bytes>1MiB` finds what was written as
`size`. The grid of elements offers a column per name in use, headed with the label and
aligned by its shape.

**The drawing reads typed values.** The GUI's node keeps the property as a JSON value, not a
string; the card and the inspector ask the vocabulary for the shape of each name and draw it
as the table says. The card measures a property from its shape (a chip row is one line, a
hash is one line, `code` is its lines) where it now wraps a string, so the height a card is
given still comes from what it draws. In the inspector's *Details*, a reference chip and a
uri open as they do from anywhere else in the window
([ADR-0066](0066-the-case-is-addressed-by-references.md),
[ADR-0044](0044-filesystem-panels-read-through-a-vfs.md)); *Copy* stays on every row and
copies the stored value, not the drawn one. Colours and icons come from the theme's tokens
and the components already in use (`Tag` for `category` and `flag`); no colour is added, and a
namespace's icon and colour are given in the same way a type namespace's are.

**The seed is a small, chosen set.** It is what an analyst records about things, written
here, not counted. Where a name has a counterpart in STIX 2.1, its row says which, so that an
export can write the property of the object and not only `x_reactor`
([stix.rs](../../crates/reactor-db/src/interchange/stix.rs)); a name without one has none.

| Name | What it says | STIX |
|---|---|---|
| `text.description` | A short description that does not need a note of its own (aliases `comment`, `remark`) | `description` |
| `text.reason` | Why it was recorded or judged so | |
| `text.version` | A version string of a file, tool or protocol | `version` |
| `text.mime_type` | What kind of content a file or blob is (aliases `mime`, `media_type`, `content_type`) | `mime_type` |
| `time.first_seen` | The earliest sighting in the evidence (alias `first_observed`) | `first_seen` |
| `time.last_seen` | The latest sighting in the evidence (alias `last_observed`) | `last_seen` |
| `time.ctime` | When a file or key was created, as the source says | `ctime` |
| `time.mtime` | When it was last changed, as the source says | `mtime` |
| `time.compiled` | When an executable was built, as its header says | `time_date_stamp` |
| `time.window_from`, `time.window_to` | The span a search covered | |
| `duration` | How long it lasted (alias `elapsed`) | |
| `size` | How big a file, blob or transfer is (aliases `size_bytes`, `file_size`, `bytes`) | `size` |
| `number.count` | How many times (alias `occurrences`) | `number_observed` |
| `number.port` | A port; `.src` and `.dst` for the two ends of a connection | `src_port`, `dst_port` |
| `number.exit_code` | How a run ended (aliases `exit_status`, `returncode`) | |
| `number.entropy` | Shannon entropy of a section or file | `entropy` |
| `id.pid` | The process id of a run | `pid` |
| `id.sid` | A Windows security identifier, or a user id | `user_id` |
| `flag.hidden` | Hidden from a normal listing | `is_hidden` |
| `flag.privileged` | Holds more rights than usual | `is_privileged` |
| `flag.defanged` | The value was written defanged (`hxxp://`), as it was found | `defanged` |
| `ref.see` | Other places worth a look, as references | |
| `uri.cwd` | The working directory of a run | `cwd` |
| `id.imphash` | The import hash of an executable | `imphash` |
| `code.command_line` | The command as it was run (aliases `cmdline`, `command`, `cmd`) | `command_line` |
| `code.raw` | The line or record as logged, before any reading of it (alias `raw_line`) | |
| `code.pattern` | A detection pattern; `code.pattern.yara`, `code.pattern.sigma` and so on | `pattern` |
| `code.query` | What a search asked, as written | |
| `category.technique` | ATT&CK technique ids, such as `T1059.001` (alias `mitre`) | `external_references` |
| `category.tactic` | ATT&CK tactics, or a kill-chain phase | `kill_chain_phases` |
| `text.alias` | Other names a thing is known by | `aliases` |
| `category.severity` | How bad the source rates it | |
| `category.protocol` | What was spoken | `protocols` |

A digest of a file is an entity (`file.hash.sha256`), not a property. `id.sha256` is for a
digest of something that is not an element of its own: a section of an executable, an attachment
inside a message.

`ref.see` is how a loose pointer is written when it is not worth a claim of its own. It does
not replace a role: a link that the graph should follow, count or contradict is still an edge.

## Consequences

- A schema step: `field_namespace`, `known_field` and `field_alias`, seeded in the style of
  `known_type`. `props` itself does not change, so existing elements keep their JSON. The
  names already in a case are not rewritten; the alias fold applies to writes from then on,
  and a `graph_query` report lists the names in use that have no namespace, so the person
  can fold them.
- A version differing only by an alias is no longer a version. That is a change to the
  fingerprint's input, so the schema version moves with it; the project is in beta and old
  cases are recreated, as in ADR-0066.
- An export carries the known names a case uses, so a report is drawn the same elsewhere;
  STIX projection writes a property under the STIX name where the seed gives one and the
  element's type is the matching object, and keeps all of them in `x_reactor` as now.
- The card's height depends on the shapes, so the tests of the card's measure (they
  assert the heights of the existing sizes) gain a case per shape.
- A property of the agent's that is wrong in kind (a path under `size`) shows the mismatch
  instead of hiding it. That is wanted, and it is the only place the drawing disagrees with
  the stored value.
- `CONTEXT.md` gains *Property namespace* and *Shape*.

## Left for later

- Shapes that need a parser of their own (a CIDR, a port range, a geographic coordinate to
  put on a map), once they are used.
- What a sub-namespace can add to its shape: `code.sql`, `code.yara` as the language to
  highlight, and the same for `number.latitude`. The names are allowed now; what they do is
  not decided.
- Editing a property in the inspector with a control for its shape (a date picker for
  `time`). The first step draws; the person edits as text, as now.
- The integration interface of ADR-0066, where a program registering a reference scheme could
  also register names under a namespace.

## Considered and rejected

- **Flat names with a shape on each** (`host`, `size`, `first_seen`, each defined). The first
  draft. Rejected: every name the agent makes up would be unknown, and so unshaped, until
  someone defined it. A namespace gives a name its drawing the moment it is written.
- **Enums, with their values defined.** Rejected: the values would have to be defined before
  they were used, which is the thing to avoid. A `category` is drawn the same, takes any text, and
  is coloured only if the case coloured it.
- **A schema per type**, or names that belong to the types they are usually found on. It is
  the familiar shape and would validate. Rejected: a size is a size whatever it is the size of,
  and the namespaces are the same for every element; a type that had to list its properties
  would be heavier to define than it is.
- **A namespace for each way of drawing** (`count` beside `number`, `hash` beside `id`, `url`
  beside `path`). Rejected: each pair differs only in grouping, shortening or which program
  opens it, and a namespace is kept only if it changes how a value is read. `uri` is read by its
  scheme, `id` shortens what is long, `number` is a number.
- **Fields for the things the types already name** (`host`, `user`, `ip`, `path`, `sha256`).
  They would be a second, weaker way to say what an entity says, one that cannot be merged,
  sourced or contradicted, and a model offered both would write the property and skip the
  element. The two are kept apart by refusing a name that is a type, a type alias or the root
  of a type.
- **A suffix convention** (`_at`, `_ref`, `_size`) to guess the shape. Rejected: the
  namespace is read before the name and the model sees it as it writes, where a suffix has
  to be remembered at the end; and the same convention would have to be kept as a table.
- **Rejecting unknown names.** It would end the drift at once. Rejected: the agent that has a
  real fact to record would be stopped or would put it in the text, where it cannot be found.
  A namespace is what makes a new name harmless, and a notice about the rest is enough.
- **Folding aliases when drawing only.** Nothing stored changes. Rejected: a filter, an
  export and a comparison between versions would still see several names.
- **Putting the shape in the value** (`{"$type": "size", "value": 184320}`). Rejected: it
  makes every write longer for the model, and the shape is a property of the name, not of
  one value.
- **Detecting the shape from the value**, with no catalogue, or for a name with no
  namespace. It needs no seed. Rejected: `184320` is a number or a size, and `2026-03-12` is a
  time or a text, depending on the name; a guess that is right most of the time draws a
  property differently from one case to the next, and nothing would fix the names. A name
  with no namespace is text, and says so in the write.
- **Seeding from a count of names agents already used.** Rejected: the names are the agent's
  own choice either way, and a count would also have recorded the overlap with the types as
  if it were wanted.
