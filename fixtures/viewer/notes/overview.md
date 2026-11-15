# Reference to variations

This is an entirely local workflow. **No image service is called.** The first
step draws an original geometric reference; the group makes two color variations.

## Read the workflow

- Follow the image from **Reference image** into each variation.
- Open the imported workflow to read its own note.
- Select any note for its full text in the existing details panel.

```yaml
with:
  image: ${{ steps.seed.outputs.image }}
```

The expression above is documentation and stays literal. Files are read by the
engine when a new plan is prepared. A saved run keeps the documentation captured
at its start, even if this Markdown file is edited later.

[External documentation](https://example.com/notes)

<script>window.fx_note_script_executed = true</script>

![A note must not fetch remote images](https://example.com/note-image.png)
