# FX www

The standalone `https://grida.co/fx` website: Next.js App Router, TypeScript,
Tailwind 4 and a static export. It owns its header and footer and links back to
[Grida home](https://grida.co/home). Large recorded-run assets stay outside Git.

## First-time setup

From the FX repository root, with Node 20.9+ and Bun 1.4.0:

```sh
bun --no-env-file install --frozen-lockfile
bun --no-env-file run dev:www
```

Open `http://127.0.0.1:3100/fx`. Ctrl-C stops the server; source edits refresh
the page. The landing artwork is included. **Working workflow demos also require
an asset source**: set the CDN base below, or prepare the ignored local media
using the [showcase guide](docs/showcases.md#prepare-reviewed-content).
Without either, the source still builds, but workflow media and Canvas cannot load.
No provider account or workflow execution is needed for website development.

## Build and preview

```sh
bun --no-env-file run check:www
bun --no-env-file run test:www
bun --no-env-file run build:www
bun --no-env-file run preview:www
```

These checks and builds work offline with the committed frozen metadata; they do
not download assets, execute workflows or publish anything. The preview serves
`out/` at `http://127.0.0.1:3100/fx` without Next.js. Use `PORT=3101` to change
the preview port. Choose the asset base **before building**, as described below.

`site.config.ts` owns the `/fx` prefix and public links. A production host must
mount the contents of `out/` under `/fx/`, including `/fx/_next/`. Hosting, domain
routing and replacing the existing Grida route are separate deployment work.
The app lives at root `www/`; update any hosting Root Directory setting to `www`
when moving an existing configuration.
See [Next static exports](https://nextjs.org/docs/app/guides/static-exports) and
[basePath](https://nextjs.org/docs/app/api-reference/config/next-config-js/basePath).

## CDN assets

Set the public delivery root before starting development or building:

```sh
# From the FX repository root; /root is an optional prefix on the asset host.
export FX_SHOWCASE_ASSET_BASE=https://assets.example.com/root
bun --no-env-file run dev:www
# Or build a static site with that same asset source.
bun --no-env-file run build:www
```

Any public HTTPS host can serve the prepared delivery tree. Preserve its relative
paths and bytes. The base names the delivery root, **not** a `showcases/<digest>`
or `runs/<digest>` directory; it contains no credentials. Its default is `/fx`,
which works only when local media has been prepared under `public/showcases/`
and `public/runs/`. Those directories are ignored, not included in a checkout.

Configure `FX_SHOWCASE_ASSET_BASE` in the hosting platform's build environment
(on Vercel, Project Settings → Environment Variables), then rebuild/redeploy.
No `NEXT_PUBLIC_` alias is needed: the build supplies the public URL to the client.
Restart development or rebuild after changing it. Hostnames are outside the
frozen content contract: migrating CDNs copies identical files, changes this
setting and rebuilds the website, without rerunning workflows.

The asset host must support public GET/HEAD, correct MIME types, CORS allowing
the website origin, and byte ranges for videos. Cross-origin JSON is checked
against its pinned hash before rendering. For a lean CDN build, move ignored
local `public/showcases/` and `public/runs/` copies aside; Next otherwise copies
them into `out/`. Preserve them outside `public/` if needed for local preview.

### Direct browser delivery policy

Prefer native `<img>`. Every `next/image` use must include a literal
`unoptimized` or `unoptimized={true}`, even though optimization is disabled
globally. Keep `images.remotePatterns` and `images.domains` empty, including
wildcards. Do not add custom image loaders, asset rewrites, proxies or server
relays. Keep `next/image` imports and props explicit: the check rejects opaque
wrappers, helper imports and prop spreads. Use `<img>` for reusable wrappers.

`check:www` and the build run the image-policy guard; run it alone with:

```sh
bun --no-env-file run --cwd www check:images
```

Tests verify explicit unoptimized URLs and rejection of accidental optimized
external images, including production optimizer URL validation. For a candidate:

1. Preview the exact built output. Open the workflow index, a demo and Canvas.
2. Inspect browser Network and image `src`/`srcset`: external media and JSON must
   request the configured CDN directly, without `/_next/image?url=...` or a
   same-origin proxy.
3. Confirm no CORS errors and exercise video playback/seeking after host changes.

These external asset bytes bypass Vercel optimization and delivery. HTML,
JavaScript, local branding/landing media and generated social previews remain
ordinary website traffic.

## Source and content ownership

- `src/app/`: landing, workflow index, eight detail pages and metadata.
- `src/lib/showcases/` and `content/showcases/`: private website adapters and
  small frozen content pins; see [showcase maintenance](docs/showcases.md).
- `../web/fx-web` and `../web/fx-react`: reusable export reader, canvas,
  class-owned interaction and thin React adapters. The website imports these;
  FX's engine knows nothing about this catalog or its CDN.
- `public/brand/` and `public/landing/`: curated local artwork. Preserve the
  [brand license](public/brand/LICENSE.txt) and [landing artwork notes](public/landing/README.md).

Follow root `AGENTS.md`: nontrivial interaction belongs in independently testable
TypeScript classes; React mounts and presents them. Bundle Lucide icons. The
landing reel shows each showcase's published result beside its recorded run, drawn by
the same canvas as the showcase pages. No workflow runs inside the page.

## Search, branding and deployment

Metadata uses Grida's origin, publisher and `@grida_co` identity, with canonical,
Open Graph and Twitter fields for each page. All pages share the build-generated
`social-preview.png`, rendered from local artwork without remote requests. Keep
`public/landing/landscape-v1.webp`: it remains the social-preview source.

FX owns `https://grida.co/fx/sitemap.xml`. It lists the landing, the workflow
index and one page per workflow from the catalog that generates them, at their
canonical URLs and without dates. The build fails if an indexable exported page is
missing, a listed URL is not one, or a page's canonical differs from its entry.
Guide pages belong to the docs sitemap, not this one. The root Grida site's
`robots.txt` must reference this sitemap; a nested app's own `robots.txt` is ignored.
The neutral palette, logo position and serif `f(x)` motif follow Grida's website;
source references and reuse basis are recorded in the asset notes above.
System fonts and webpack keep builds independent of remote fonts and development
worker port requirements. There is no shadcn or Grida monorepo dependency.
