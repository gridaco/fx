import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";
import sharp from "sharp";

export const dynamic = "force-static";
const size = { width: 1200, height: 630 };

/** Build from local artwork; social previews require no runtime or remote font fetch. */
export async function GET() {
  // ImageResponse's SVG renderer needs PNG/JPEG data rather than WebP.
  const landscape = await sharp(await readFile(join(process.cwd(), "public/landing/landscape-v1.webp"))).png().toBuffer();
  const mark = await readFile(join(process.cwd(), "public/brand/grida-mark.svg"), "utf8");
  return new ImageResponse(
    <div style={{ display: "flex", width: "100%", height: "100%", background: "#fafafa", color: "#171717" }}>
      <div style={{ display: "flex", flexDirection: "column", justifyContent: "space-between", width: 580, padding: "56px 48px 48px 56px" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
          <img src={`data:image/svg+xml;base64,${Buffer.from(mark.replace('fill="currentColor"', 'fill="#171717"')).toString("base64")}`} width={32} height={32} alt="" />
          <span style={{ fontSize: 30, fontWeight: 700 }}>Grida FX</span>
        </div>
        <div style={{ display: "flex", flexDirection: "column" }}>
          <span style={{ fontSize: 20, color: "#737373", marginBottom: 24 }}>ASSET WORKFLOWS, AS CODE</span>
          <span style={{ fontSize: 86, lineHeight: 0.98, fontWeight: 700, letterSpacing: -5 }}>Go with</span>
          <span style={{ fontSize: 86, lineHeight: 0.98, fontWeight: 700, letterSpacing: -5 }}>the flow.</span>
          <span style={{ fontSize: 25, color: "#525252", marginTop: 28 }}>Plan. Run. Inspect. Reuse.</span>
        </div>
        <span style={{ fontSize: 23, color: "#737373" }}>grida.co/fx</span>
      </div>
      <div style={{ display: "flex", width: 620, height: 630, padding: "24px 24px 24px 0" }}>
        <img src={`data:image/png;base64,${landscape.toString("base64")}`} width={596} height={582} alt="" style={{ objectFit: "cover", borderRadius: 20 }} />
      </div>
    </div>,
    size,
  );
}
