# Workflow-authored video previews

An actual node step may select one of its video output ports for its canvas preview and,
optionally, one of its image output ports as that video's poster:

```yaml
steps:
  render:
    uses: ./nodes/movie.py#render
    preview: { video: clip, poster: cover }
```

The declaration belongs to the workflow step, not the node type. It is an optional object
with required `video` and optional `poster`, and no other members. Each value is a literal
output-port name matching `^[a-z][a-z0-9_]*$`. Expressions, file paths, URLs, cross-step
references, list indices and collection keys are not selectors. A group or imported workflow
step cannot declare `preview`; a repeated node step can.

The engine validates the declaration against the resolved node type while planning:

- `video` names a declared singleton output in the `video` family.
- `poster`, when present, names a declared singleton output in the `image` family.
- Optional singleton ports are allowed. Lists and keyed collections are not.
- Missing ports and wrong families or shapes are planning problems. An unresolved type does
  not supply guessed output declarations.

There are no node-type preview defaults in this version. A declaration without `poster`
explicitly chooses a video for the existing native first-frame preview. A declaration with a
poster associates the two outputs of the same instance and take; it does not assert that their
contents correspond. The author owns that choice. Optional outputs may be absent at runtime:
the viewer uses only the available recorded files and follows [the canvas rules](layout.md).
No preview declaration causes a node, provider call, file conversion or image extraction to run.

## Recording and identity

The optional `preview` object is recorded unchanged in the authored step metadata and in every
expanded instance, including each repeated item and take. A not-yet-expanded repeat retains
the declaration in its step metadata. `node_started` and known-type `node_failed` or
`node_skipped` metadata carry the same object, so an instance first created at runtime keeps
its authored choice. The field is omitted when the step has no declaration; old records
therefore retain automatic preview selection.

The object holds port names, never URLs, local paths, additional output values or invented
artifact references. Readers resolve it through that same instance's recorded outputs and
verified artifact inventory. Viewing saved data never evaluates workflow expressions, opens
author code or infers associations from filenames or matching digests. Export inclusion and
disclosure remain governed by [exports.md](exports.md).

`preview` is presentation metadata, outside step identities and call keys. It is not an input
or parameter and creates no `reads`, `needs`, binding, phase or scheduling dependency. The
authored workflow document remains part of the plan digest: changing the declaration changes
that digest and requires a new run folder, while identical work can still reuse caches
([identity.md](identity.md) §§8–10). No existing digest algorithm changes.
