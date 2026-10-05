# cache-replay

The project's store, `in/.fx/cache`, is seeded with one paid call and the file it returned, as
if an earlier live run had made it. The run in this case makes no call and needs no `--live`:
the trusted call record answers the call and bills nothing (`spec/store.md` section 4).
`cache-replay-miss` is the same project without `.fx/`, so its run must refuse the paid call.

Nothing in the store is a secret. A call record keeps the canonical request, and keys never
travel in requests. (The repository's `.gitignore` ignores only the root `/.fx/`, so this one
is committed.)

## The call key

Computed with `spec/identity.md` sections 7 and 9, using the Python `rfc8785` package for the
canonical JSON: `digest(v) = sha256(rfc8785.dumps(v)).hexdigest()`.

The route `img-a@acme` serves `image.generate` in `in/routes.yaml` and declares no contract.
Its fingerprint is the route example of `spec/identity.md` section 14:

    canon  = {"capability":"image.generate","contract":{},"kind":"fx-route-v1","model":"img-a","provider":"acme"}
    digest = 4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4

The step is `draw: {uses: fx/image.generate@1, with: {prompt: a lantern}}`. Planning fills in
the param defaults `background: auto` and `vars: {}`. `vars` is spent on rendering the prompt
and is not sent, so the canonical request is `{"prompt": "a lantern", "background": "auto"}`.
The first take is `[1]`:

    canon  = {"capability":"image.generate","kind":"fx-call-v1","request":{"background":"auto","prompt":"a lantern"},"route":"4d41b81c56215efdd18574eab8e2b704a8ecc86af08d569f55cd29973c4e7ed4","take":[1]}
    key    = 3aa41bf6e466138e920882b87c9b7ef9fc22dc245a2861c121f80adaaafd066d

## The files

`in/.fx/cache/calls/3a/3aa41bf6e466138e920882b87c9b7ef9fc22dc245a2861c121f80adaaafd066d.json`
is the `fx-call-record-v1` record, written as `canon(record)` (`spec/store.md` section 3):

- `key`: the key above;
- `capability`: `image.generate`;
- `route`: `{"id": "img-a@acme", "fingerprint": "4d41b81c…"}`;
- `request`: the canonical request above;
- `take`: `[1]`;
- `files`: one file, `image`: `{"digest": "f3945de0…", "kind": "image/png", "name": "image", "size": 70}`;
- `data`: null;
- `cost_usd`: 0.02, what the original call was charged. Replaying it charges nothing.

Recomputing `call_key` from the record's `capability`, `route.fingerprint`, `request` and
`take` gives its `key`, as `spec/store.md` section 3 requires.

`in/.fx/cache/files/f3/f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4` is the
stored file: a 1x1 opaque RGBA PNG of 70 bytes (one pixel, `(255, 200, 0, 255)`), sha256
`f3945de0c1182a1b279816f51ef2e79938d04957c7bfde94cd4bf2eb4c2170b4`. It was written by the
standard-library snippet in `facts/README.md`, with one row of one pixel:
`rows = [[(255, 200, 0, 255)]]` and `struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)`.

## What the case pins

- `run` without `--live` exits 0, and `project.json` shows the step succeeded with nothing
  charged and the run's output `image` with digest `f3945de0…`.
- A second run of the same plan (`project-two.json`) is answered by the result cache that the
  first run wrote.
