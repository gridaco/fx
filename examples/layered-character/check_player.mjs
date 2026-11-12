#!/usr/bin/env node
// Check an exported package against an independently built upstream player.
// The player, browser, packages and generated review artifacts stay outside Git.
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile, mkdir, writeFile, realpath, stat } from "node:fs/promises";
import { dirname, extname, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { inflateRawSync } from "node:zlib";

const help = `Usage: node check_player.mjs --player-root PATH --package FILE.livliv --out DIR
  --player FILE       Built browser ES module (alternative to --player-root).
  --playwright PATH   Installed playwright-core or playwright package directory.
                     Otherwise resolve it from this example or --player-root.
  --browser FILE      Chromium executable; otherwise use Playwright's browser.
  --profile NAME      full (default), portrait, gaze, or puppet (head/body too).
  --neutral-reference FILE.png
                     Compare the first neutral frame to this native-size source.
  --no-video          Skip the optional FFmpeg animated preview.

Install playwright-core and its Chromium browser before running this check.
The upstream player must already be built. This check makes no provider calls.
The output contains frames/, contact-sheet.html, contact-sheet.png, report.json,
and preview.mp4 when FFmpeg is available. Browser network access stays local.
Source comparison writes neutral-native.png and neutral-diff.png. Its exactness
result is separate from technical playback; naturalness is not assessed here.
`;

function options(args) {
  const result = { profile: "full" };
  const valued = new Set(["player-root", "player", "package", "out", "playwright", "browser", "profile", "neutral-reference"]);
  for (let i = 0; i < args.length; i++) {
    const key = args[i].replace(/^--/, "");
    if (args[i] === "--help" || args[i] === "-h") {
      console.log(help);
      process.exit(0);
    }
    if (args[i] === "--no-video") result.no_video = true;
    else if (args[i].startsWith("--") && valued.has(key) && args[i + 1] && !args[i + 1].startsWith("--")) result[key] = args[++i];
    else throw new Error(`Unknown or incomplete argument: ${args[i]}`);
  }
  if ((!result.player && !result["player-root"]) || !result.package || !result.out) throw new Error(help);
  if (!["full", "portrait", "gaze", "puppet"].includes(result.profile)) throw new Error("--profile must be full, portrait, gaze or puppet.");
  return result;
}

const args = options(process.argv.slice(2));
const playerFile = await realpath(resolve(args.player ?? join(args["player-root"], "dist/index.js")));
const playerDir = dirname(playerFile);
const packageFile = await realpath(resolve(args.package));
const packageBytes = await readFile(packageFile);
const referenceFile = args["neutral-reference"] ? await realpath(resolve(args["neutral-reference"])) : null;
const out = resolve(args.out);
await mkdir(join(out, "frames"), { recursive: true });

// Inspect the same generated package the player receives, using its ZIP directory.
// No upstream example artwork or private renderer state is used for gaze evidence.
function zipEntries(bytes) {
  let end = bytes.length - 22;
  for (; end >= Math.max(0, bytes.length - 65_557); end--) if (bytes.readUInt32LE(end) === 0x06054b50) break;
  if (end < 0 || bytes.readUInt32LE(end) !== 0x06054b50) throw new Error("No ZIP directory in package.");
  const entries = new Map();
  const count = bytes.readUInt16LE(end + 10);
  if (count > 2048) throw new Error("Too many ZIP entries to inspect.");
  let totalSize = 0;
  let at = bytes.readUInt32LE(end + 16);
  for (let i = 0; i < count; i++) {
    if (bytes.readUInt32LE(at) !== 0x02014b50) throw new Error("Invalid ZIP directory.");
    const flags = bytes.readUInt16LE(at + 8), method = bytes.readUInt16LE(at + 10);
    const length = bytes.readUInt32LE(at + 20), nameLength = bytes.readUInt16LE(at + 28);
    const expectedSize = bytes.readUInt32LE(at + 24);
    totalSize += expectedSize;
    if (totalSize > 256 * 1024 * 1024) throw new Error("Package exceeds inspection size limit.");
    const name = bytes.subarray(at + 46, at + 46 + nameLength).toString("utf8");
    const offset = bytes.readUInt32LE(at + 42);
    if (flags & 1) throw new Error("Encrypted packages cannot be inspected.");
    if (bytes.readUInt32LE(offset) !== 0x04034b50) throw new Error("Invalid ZIP entry.");
    const start = offset + 30 + bytes.readUInt16LE(offset + 26) + bytes.readUInt16LE(offset + 28);
    const compressed = bytes.subarray(start, start + length);
    if (method !== 0 && method !== 8) throw new Error("Only stored or deflated ZIP entries can be inspected.");
    const data = method === 8 ? inflateRawSync(compressed, { maxOutputLength: 256 * 1024 * 1024 }) : compressed;
    if (data.length !== expectedSize) throw new Error("ZIP entry size differs from its directory.");
    entries.set(name, data);
    at += 46 + nameLength + bytes.readUInt16LE(at + 30) + bytes.readUInt16LE(at + 32);
  }
  return entries;
}

const proofMasks = new Map();
let proofEyes = [];
let proofGeometry = null;
if (["gaze", "puppet"].includes(args.profile)) {
  const entries = zipEntries(packageBytes);
  const modelName = [...entries.keys()].filter((name) => name === "model.json" || name.endsWith("/model.json")).sort((a, b) => a.length - b.length)[0];
  if (!modelName) throw new Error("Gaze evidence needs model.json in the package.");
  const model = JSON.parse(entries.get(modelName).toString("utf8"));
  const prefix = modelName.slice(0, -"model.json".length);
  proofEyes = model.layers.filter((layer) => ["eye_L", "eye_R"].includes(layer.role) && layer.mask).map((layer, index) => {
    const iris = model.layers.find((candidate) => candidate.clip === layer.id && candidate.role?.startsWith("iris_"));
    if (!iris?.range || !entries.has(prefix + layer.mask)) throw new Error("Gaze evidence needs clipped irises with movement ranges and included eye masks.");
    const url = `/proof-mask/${index}`;
    proofMasks.set(url, entries.get(prefix + layer.mask));
    return { id: layer.id, role: layer.role, box: [layer.x, layer.y, layer.w, layer.h], mask: url, iris: iris.id, range: iris.range };
  });
  if (proofEyes.length !== 2) throw new Error("Gaze profile needs two masked eye layers and their clipped iris layers.");
  if (args.profile === "puppet") {
    const facial = model.layers.filter((layer) => /^(eye_|iris_|lid_|shine_)/.test(layer.role ?? "") || layer.role === "mouth");
    const head = model.layers.filter((layer) => layer.group === "head");
    const body = model.layers.filter((layer) => layer.group === "body");
    if (!head.length || !body.length || facial.some((layer) => layer.group !== "head")) throw new Error("Puppet profile needs separate head/body layers and facial features in the head group.");
    proofGeometry = {
      anchors:model.anchors,canvas:model.canvas,
      mouth_point:facial.find((layer)=>layer.role==="mouth")?.pivot ?? [model.anchors.head.cx,model.anchors.face[1]+(model.anchors.face[3]-model.anchors.face[1])*0.72],
      head_layers:head.map((layer) => ({id:layer.id,role:layer.role ?? "plain",depth:layer.depth ?? 0})),
      body_layers:body.map((layer) => layer.id),
      facial_features_share_head_group:true,
    };
  }
}

async function playwright() {
  if (args.playwright) {
    const require = createRequire(pathToFileURL(join(resolve(args.playwright), "package.json")));
    return require(resolve(args.playwright));
  }
  const bases = [import.meta.url];
  if (args["player-root"]) bases.push(pathToFileURL(join(resolve(args["player-root"]), "package.json")));
  for (const base of bases) {
    const require = createRequire(base);
    for (const name of ["playwright-core", "playwright"]) {
      try { return require(name); } catch (error) {
        if (error.code !== "MODULE_NOT_FOUND") throw error;
      }
    }
  }
  throw new Error("Install playwright-core beside this script or the player, or pass --playwright PATH.");
}

const html = `<!doctype html><html lang="en"><meta charset="utf-8">
<title>Layered character compatibility check</title><link rel="icon" href="data:,">
<style>body{margin:0;background:#e8edf1}canvas{display:block;width:480px;height:640px}</style>
<canvas id="character"></canvas><script type="module">
import {Player, defaultParams} from '/player/${encodeURIComponent(playerFile.slice(playerDir.length + 1))}';
const canvas = document.querySelector('canvas');
const profile = ${JSON.stringify(args.profile)};
const proofEyes = ${JSON.stringify(proofEyes)};
const proofGeometry = ${JSON.stringify(proofGeometry)};
const presentation = profile === 'puppet' ? {width:720,height:960,zoom:1,head:true,y:0.055} : profile === 'gaze' ? {width:720,height:960,zoom:1.18,head:true,y:0} : {width:480,height:640,zoom:1,head:false,y:0};
const createPlayer = (native = false) => {
  if (!native) {canvas.style.width=presentation.width+'px';canvas.style.height=presentation.height+'px';}
  const p = new Player(canvas, {idle:false,followPointer:false,autoplay:false,maxPixelRatio:1,lang:'en'});
  if (!native) Object.assign(p.view,{zoom:presentation.zoom,head:presentation.head,y:presentation.y});
  return p;
};
let player = createPlayer();
const info = await player.load('/package.livliv');
const scratch = document.createElement('canvas');
const context = scratch.getContext('2d', {willReadFrequently:true});
let baseline;
const captured = new Map();
const defaults = defaultParams();
for (const eye of proofEyes) {
  eye.image = new Image(); eye.image.src=eye.mask; await eye.image.decode();
}
const settle = async (milliseconds) => {
  player.play();
  const end = performance.now() + milliseconds;
  while (performance.now() < end) await new Promise(requestAnimationFrame);
  player.pause();
};
function measurePuppet(pixels, reference, params) {
  if (!proofGeometry) return undefined;
  const w=canvas.width,h=canvas.height,A=proofGeometry.anchors;
  const [x0,y0]=player.canvasToModel(0,0),[x1,y1]=player.canvasToModel(canvas.clientWidth,canvas.clientHeight);
  const project=(x,y)=>[(x-x0)*w/(x1-x0),(y-y0)*h/(y1-y0)];
  const faceWidth=A.face[2]-A.face[0],faceHeight=A.face[3]-A.face[1],bodyHeight=A.bottom_y-A.neck_y;
  const track=(name,source,range=48,radius=10)=>{
    const [px,py]=project(...source).map(Math.round),samples=[];
    for(let dy=-radius;dy<=radius;dy+=3)for(let dx=-radius;dx<=radius;dx+=3){
      const x=px+dx,y=py+dy;
      if(x>=0&&x<w&&y>=0&&y<h&&reference[4*(y*w+x)+3]>200)samples.push([dx,dy,4*(y*w+x)]);
    }
    if(samples.length<12)return {name,source,visible:false};
    let best={score:Infinity,dx:0,dy:0,error:Infinity};
    const tryOffset=(dx,dy)=>{
      let error=0;
      for(const [u,v,i]of samples){
        const x=px+u+dx,y=py+v+dy;
        if(x<0||x>=w||y<0||y>=h)return;
        const j=4*(y*w+x);
        for(let k=0;k<3;k++)error+=(pixels[j+k]-reference[i+k])**2;
      }
      error/=samples.length*3;
      const score=error+0.0001*(dx*dx+dy*dy);
      if(score<best.score)best={score,dx,dy,error};
    };
    const step=range>12?3:1;
    for(let dy=-range;dy<=range;dy+=step)for(let dx=-range;dx<=range;dx+=step)tryOffset(dx,dy);
    const coarse={...best};
    for(let dy=coarse.dy-2;dy<=coarse.dy+2;dy++)for(let dx=coarse.dx-2;dx<=coarse.dx+2;dx++)tryOffset(dx,dy);
    return {name,source,screen:[px,py],visible:true,dx:best.dx,dy:best.dy,rms_rgb_error:Math.sqrt(best.error)};
  };
  const points=[
    track('nose',[A.head.cx,A.face[1]+faceHeight*0.55]),
    track('mouth',proofGeometry.mouth_point),
    track('lower_jacket',[A.head.cx,A.bottom_y-bodyHeight*0.08],8,12),
  ];
  if((params.eyeLOpen??1)===1&&(params.eyeROpen??1)===1&&!params.eyeBallX&&!params.eyeBallY)for(const eye of proofEyes)points.push(track(eye.id,[eye.box[0]+eye.box[2]/2,eye.box[1]+eye.box[3]/2]));
  const head=points.filter((point)=>point.name!=='lower_jacket'&&point.visible),root=points.find((point)=>point.name==='lower_jacket');
  const dx=head.reduce((sum,point)=>sum+point.dx,0)/Math.max(1,head.length),dy=head.reduce((sum,point)=>sum+point.dy,0)/Math.max(1,head.length);
  const neckTop=project(A.head.cx-faceWidth*0.05,A.neck_y-8),neckBottom=project(A.head.cx+faceWidth*0.05,A.neck_y+16);
  let neckSamples=0,neckTransparent=0;
  for(let y=Math.max(0,Math.ceil(neckTop[1]));y<Math.min(h,Math.floor(neckBottom[1]));y++)for(let x=Math.max(0,Math.ceil(neckTop[0]));x<Math.min(w,Math.floor(neckBottom[0]));x++){
    neckSamples++;if(pixels[4*(y*w+x)+3]<32)neckTransparent++;
  }
  const headBottom=Math.max(0,Math.min(h,Math.round(project(A.head.cx,A.neck_y+bodyHeight*0.12)[1])));
  let headBorderPixels=0;
  for(let x=0;x<w;x++)if(pixels[4*x+3]>8)headBorderPixels++;
  for(let y=1;y<headBottom;y++)for(const x of [0,w-1])if(pixels[4*(y*w+x)+3]>8)headBorderPixels++;
  return {
    tracking:points,
    head_translation_estimate:[dx,dy],
    lower_jacket_translation:root.visible?[root.dx,root.dy]:null,
    head_relative_to_lower_jacket:root.visible?head.reduce((sum,point)=>sum+Math.hypot(point.dx-root.dx,point.dy-root.dy),0)/Math.max(1,head.length):null,
    neck_core:{source_box:[A.head.cx-faceWidth*0.05,A.neck_y-8,faceWidth*0.1,24],samples:neckSamples,transparent_pixels:neckTransparent},
    head_border_pixels:headBorderPixels,
  };
}
window.review = {
  info,
  async compareNeutral() {
    const reference = new Image();
    reference.src = '/neutral-reference.png';
    await reference.decode();
    const width = reference.naturalWidth, height = reference.naturalHeight;
    if (width !== info.size[0] || height !== info.size[1]) throw new Error('Neutral reference dimensions differ from the model canvas.');
    // First-frame comparison: native pixels, DPR 1, no animation or physics time.
    player.dispose();
    canvas.style.width = width + 'px'; canvas.style.height = height + 'px';
    player = createPlayer(true);
    await player.load('/package.livliv');
    Object.assign(player.params, defaults);
    const image = player.snapshot();
    scratch.width = width; scratch.height = height;
    context.clearRect(0,0,width,height); context.drawImage(canvas,0,0);
    const actual = context.getImageData(0,0,width,height).data;
    context.clearRect(0,0,width,height); context.drawImage(reference,0,0);
    const expected = context.getImageData(0,0,width,height).data;
    const diff = context.createImageData(width,height);
    const summary = (channels) => ({channels,sum:0,max_abs_error:0,pixels_changed:0,pixels_over_1:0,pixels_over_8:0});
    const metrics = {premultiplied_rgba:summary(4),over_black:summary(3),over_white:summary(3)};
    for (let i=0;i<actual.length;i+=4) {
      const aa=actual[i+3]/255, ea=expected[i+3]/255;
      const errors = {premultiplied_rgba:[Math.abs(actual[i+3]-expected[i+3])],over_black:[],over_white:[]};
      for (let k=0;k<3;k++) {
        const black=Math.abs(actual[i+k]*aa-expected[i+k]*ea);
        const white=Math.abs((actual[i+k]*aa+255*(1-aa))-(expected[i+k]*ea+255*(1-ea)));
        errors.premultiplied_rgba.push(black);
        errors.over_black.push(black); errors.over_white.push(white);
        diff.data[i+k]=Math.min(255,Math.round(Math.max(black,white)*8));
      }
      diff.data[i+3]=255;
      for (const [key,values] of Object.entries(errors)) {
        const m=metrics[key], maximum=Math.max(...values);
        m.sum+=values.reduce((sum,value)=>sum+value,0);
        m.max_abs_error=Math.max(m.max_abs_error,maximum);
        if (maximum>0) m.pixels_changed++;
        if (maximum>1) m.pixels_over_1++;
        if (maximum>8) m.pixels_over_8++;
      }
    }
    const pixels=width*height;
    for (const m of Object.values(metrics)) {
      m.mean_abs_error=m.sum/(pixels*m.channels);
      m.changed_fraction=m.pixels_changed/pixels;
      m.over_1_fraction=m.pixels_over_1/pixels;
      m.over_8_fraction=m.pixels_over_8/pixels;
      delete m.sum; delete m.channels;
    }
    context.putImageData(diff,0,0);
    const difference=scratch.toDataURL('image/png');
    const exact=metrics.premultiplied_rgba.max_abs_error===0;
    player.dispose();
    player=createPlayer(); await player.load('/package.livliv');
    return {image,difference,width,height,exact,status:exact?'exact':'different',metrics};
  },
  async capture(id, params = {}, expression = null, references = [], milliseconds = 500) {
    // Reload from rest so a preceding pose's hair springs or expression cannot
    // make an unsupported control look as though it changed the drawing.
    player.dispose();
    player = createPlayer();
    await player.load('/package.livliv');
    Object.assign(player.params, defaults, params);
    if (!player.setExpression(expression)) throw new Error('Unknown expression');
    if (id !== 'neutral') await settle(milliseconds);
    const image = player.snapshot();
    scratch.width = canvas.width; scratch.height = canvas.height;
    context.drawImage(canvas, 0, 0);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    let covered = 0, changed = 0;
    const compare = references.map((reference) => ({reference, pixels:captured.get(reference), changed:0}));
    const differs = (i, other) => Math.max(Math.abs(pixels[i]-other[i]),Math.abs(pixels[i+1]-other[i+1]),Math.abs(pixels[i+2]-other[i+2]),Math.abs(pixels[i+3]-other[i+3])) > 8;
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i + 3] > 8) covered++;
      if (baseline && differs(i,baseline)) changed++;
      for (const reference of compare) if (differs(i,reference.pixels)) reference.changed++;
    }
    if (!baseline) baseline = pixels.slice();
    captured.set(id,pixels.slice());
    const total = pixels.length / 4;
    let gaze;
    if (proofEyes.length && !(params.angleX||params.angleY||params.angleZ||params.bodyX||params.bodyZ||params.breath)) {
      const maskCanvas=document.createElement('canvas'); maskCanvas.width=canvas.width;maskCanvas.height=canvas.height;
      const maskContext=maskCanvas.getContext('2d',{willReadFrequently:true});
      const [x0,y0]=player.canvasToModel(0,0), [x1,y1]=player.canvasToModel(canvas.clientWidth,canvas.clientHeight);
      const sx=canvas.width/(x1-x0),sy=canvas.height/(y1-y0);
      const union=new Uint8Array(total),centroids=[];
      for (const eye of proofEyes) {
        maskContext.setTransform(1,0,0,1,0,0);maskContext.clearRect(0,0,canvas.width,canvas.height);
        maskContext.setTransform(sx,0,0,sy,-x0*sx,-y0*sy);maskContext.drawImage(eye.image,...eye.box);
        const mask=maskContext.getImageData(0,0,canvas.width,canvas.height).data;
        let weight=0,cx=0,cy=0;
        for (let p=0;p<total;p++) if (mask[4*p+3]>8) {
          union[p]=1;
          const darkness=Math.max(0,220-(pixels[4*p]*0.2126+pixels[4*p+1]*0.7152+pixels[4*p+2]*0.0722))*(mask[4*p+3]/255);
          weight+=darkness;cx+=(p%canvas.width)*darkness;cy+=Math.floor(p/canvas.width)*darkness;
        }
        centroids.push({eye:eye.id,x:weight?cx/weight:null,y:weight?cy/weight:null,weight});
      }
      // The original open-eye mask is a conservative envelope for half blinks.
      // Compare to the same eye-open state so eyelid movement is not counted as gaze.
      const allowed=union.slice(),margin=2;
      for (let p=0;p<total;p++) if (union[p]) {
        const x=p%canvas.width,y=Math.floor(p/canvas.width);
        for (let dy=-margin;dy<=margin;dy++) for (let dx=-margin;dx<=margin;dx++) if (x+dx>=0&&x+dx<canvas.width&&y+dy>=0&&y+dy<canvas.height) allowed[(y+dy)*canvas.width+x+dx]=1;
      }
      const baselineId=id.startsWith('half_blink_gaze_')?'half_blink':id.startsWith('closed_gaze_')?'blink':'neutral';
      const reference=captured.get(baselineId);
      let inside=0,outside=0;
      if (reference) for (let p=0;p<total;p++) if (differs(4*p,reference)) {if(allowed[p])inside++;else outside++;}
      gaze={baseline:baselineId,changed_inside_eye_mask:inside,changed_outside_eye_mask:outside,mask_margin_pixels:margin,centroids};
    }
    const puppet=measurePuppet(pixels,baseline,params);
    return {image, coverage: covered / total, changed: changed / total, compared:compare.map(({reference,changed}) => ({reference,changed:changed/total})), gaze, puppet, context_lost:player.contextLost, webgl_error:canvas.getContext('webgl2').getError()};
  },
  async motion(index, count) {
    const t = index / count * Math.PI * 2;
    if (profile === 'puppet') {
      const seconds=index/24;
      const poses=[
        [0,0,0,0],[0.7,0,0,0],[1.2,-12,0,0],[2.2,-12,0,0],
        [2.8,12,0,0],[3.8,12,0,0],[4.3,0,0,0],
        [4.8,0,8,0],[5.7,0,8,0],[6.2,0,-8,0],[7.1,0,-8,0],
        [7.6,0,0,-5],[8.5,0,0,-5],[9.0,0,0,5],[9.9,0,0,5],[10.4,0,0,0]
      ];
      let a=poses[0],b=poses.at(-1);
      for(let i=1;i<poses.length;i++)if(seconds<=poses[i][0]){a=poses[i-1];b=poses[i];break;}
      const u=Math.max(0,Math.min(1,(seconds-a[0])/(b[0]-a[0]))),ease=u*u*(3-2*u);
      const blinkAt=[0.4,3.95,7.3,10.2,12.7,14.5].reduce((open,at)=>Math.min(open,Math.abs(seconds-at)<0.17?1-Math.sin(Math.PI*(seconds-at+0.17)/0.34):1),1);
      const idle=Math.max(0,seconds-10.4),enter=Math.min(1,idle/0.6);
      Object.assign(player.params,defaults,{
        angleX:seconds<10.4?a[1]+(b[1]-a[1])*ease:Math.sin(idle*0.85)*4*enter,
        angleY:seconds<10.4?a[2]+(b[2]-a[2])*ease:Math.sin(idle*1.05)*2*enter,
        angleZ:seconds<10.4?a[3]+(b[3]-a[3])*ease:Math.sin(idle*0.7)*1.2*enter,
        eyeBallX:Math.sin(idle*1.15)*0.5*enter,eyeBallY:Math.sin(idle*0.8)*0.25*enter,
        eyeLOpen:blinkAt,eyeROpen:blinkAt,breath:(0.5+0.5*Math.sin(idle*1.3))*enter,
      });
      player.setExpression(null);
      await settle(1000/24);
    } else if (profile === 'gaze') {
      const seconds=index/24;
      const poses=[
        [0,0,0],[0.7,0,0],[1.0,-1,0],[2.0,-1,0],[2.35,1,0],[3.35,1,0],
        [3.7,0,1],[4.7,0,1],[5.05,0,-1],[6.05,0,-1],[6.4,-0.8,0.7],[7.25,-0.8,0.7],
        [7.6,0.8,-0.65],[8.45,0.8,-0.65],[8.85,0,0],[12,0,0]
      ];
      let a=poses[0],b=poses.at(-1);
      for(let i=1;i<poses.length;i++) if(seconds<=poses[i][0]){a=poses[i-1];b=poses[i];break;}
      const u=Math.max(0,Math.min(1,(seconds-a[0])/(b[0]-a[0]))),ease=u*u*(3-2*u);
      const blinkAt=[0.35,8.95,11.45].reduce((open,at)=>Math.min(open,Math.abs(seconds-at)<0.16?1-Math.sin(Math.PI*(seconds-at+0.16)/0.32):1),1);
      const speech=seconds>9.25&&seconds<11.25?Math.pow(Math.max(0,Math.sin((seconds-9.25)*Math.PI*3.5)),0.75)*0.8:0;
      Object.assign(player.params,defaults,{eyeBallX:a[1]+(b[1]-a[1])*ease,eyeBallY:a[2]+(b[2]-a[2])*ease,eyeLOpen:blinkAt,eyeROpen:blinkAt,mouthOpen:speech,vowel:Math.sin((seconds-9.25)*Math.PI*1.1)*0.8});
      player.setExpression(null);
      await settle(1000/24);
    } else {
    Object.assign(player.params, defaults, {
      eyeLOpen:index % 27 < 2 ? 0 : 1, eyeROpen:index % 27 < 2 ? 0 : 1,
      mouthOpen:(Math.sin(t*7)+1)/2, vowel:Math.sin(t*3),
      ...(profile === 'full' ? {
        angleX:Math.sin(t)*22, angleY:Math.sin(t*2)*12, angleZ:Math.sin(t)*7,
        eyeBallX:Math.sin(t)*0.8, eyeBallY:Math.cos(t)*0.3, breath:(Math.sin(t*2)+1)/2
      } : {})
    });
    player.setExpression(profile === 'full' && index > count/2 && info.expressions.length ? info.expressions[0].id : null);
    await settle(70);
    }
    player.resize();
    scratch.width = canvas.width; scratch.height = canvas.height;
    const background=context.createLinearGradient(0,0,0,scratch.height);background.addColorStop(0,'#edf0ec');background.addColorStop(1,'#dce4df');
    context.fillStyle = ['gaze','puppet'].includes(profile) ? background : '#e8edf1'; context.fillRect(0,0,scratch.width,scratch.height);
    context.drawImage(canvas,0,0);
    return scratch.toDataURL('image/png');
  },
  dispose(){ player.dispose(); }
};
</script></html>`;

const types = { ".js": "text/javascript", ".mjs": "text/javascript", ".png": "image/png", ".html": "text/html" };
const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    if (pathname === "/") return response.writeHead(200, { "content-type": "text/html" }).end(html);
    if (pathname === "/package.livliv") return response.writeHead(200, { "content-type": "application/zip" }).end(packageBytes);
    if (proofMasks.has(pathname)) return response.writeHead(200, { "content-type": "application/octet-stream" }).end(proofMasks.get(pathname));
    if (pathname === "/neutral-reference.png" && referenceFile) return response.writeHead(200, { "content-type": "image/png" }).end(await readFile(referenceFile));
    if (pathname === "/contact-sheet.html") return response.writeHead(200, { "content-type": "text/html" }).end(await readFile(join(out, "contact-sheet.html")));
    let file;
    if (pathname.startsWith("/player/")) {
      const candidate = resolve(playerDir, pathname.slice(8));
      if (!candidate.startsWith(playerDir + sep)) return response.writeHead(403).end();
      file = await realpath(candidate);
      if (!file.startsWith(playerDir + sep)) return response.writeHead(403).end();
    } else if (/^\/frames\/[a-z0-9_-]+\.png$/.test(pathname)) file = join(out, pathname.slice(1));
    if (!file || !(await stat(file)).isFile()) return response.writeHead(404).end();
    response.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" }).end(await readFile(file));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
const origin = `http://127.0.0.1:${server.address().port}`;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const saveImage = (file, data) => writeFile(file, Buffer.from(data.slice(data.indexOf(",") + 1), "base64"));
const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const sanitize = (s) => String(s).replaceAll(origin, "[local]").replace(/(?:file:\/\/)?\/(?:Users|private|home|tmp)\/[^\s)"']+/g, "[local path]");
const report = {
  status: "failed",
  status_scope: "technical_playback",
  naturalness: "unassessed",
  profile: args.profile,
  neutral_reconstruction: { status: "not_requested" },
  package_sha256: sha256(packageBytes),
  player_sha256: sha256(await readFile(playerFile)),
  browser: "chromium",
  checks: [],
  console_errors: [],
  console_warnings: [],
  page_errors: [],
  failed_requests: [],
  blocked_external_requests: 0,
  video: { status: "skipped" },
};
let browser;
try {
  const { chromium } = await playwright();
  browser = await chromium.launch({
    ...(args.browser ? { executablePath: resolve(args.browser) } : {}),
    args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
  });
  report.browser_version = browser.version();
  const page = await browser.newPage({ viewport: ["gaze", "puppet"].includes(args.profile) ? { width: 720, height: 960 } : { width: 480, height: 640 }, deviceScaleFactor: 1 });
  page.on("console", (message) => {
    if (message.type() === "error") report.console_errors.push(sanitize(message.text()));
    if (message.type() === "warning") report.console_warnings.push(sanitize(message.text()));
  });
  page.on("pageerror", (error) => report.page_errors.push(sanitize(error.message)));
  page.on("requestfailed", (request) => report.failed_requests.push(sanitize(new URL(request.url()).pathname)));
  await page.route("**/*", (route) => {
    if (new URL(route.request().url()).origin === origin) return route.continue();
    report.blocked_external_requests++;
    return route.abort();
  });
  await page.goto(origin);
  await page.waitForFunction(() => !!window.review, null, { timeout: 30_000 });
  const info = await page.evaluate(() => window.review.info);
  report.model = { name: info.name, format: info.format, version: info.version, size: info.size, expressions: info.expressions.map((e) => e.id) };
  if (info.format !== "livliv-model" || info.version !== 4) throw new Error("The loaded package is not a LivLiv v4 model.");
  if (referenceFile) {
    const comparison = await page.evaluate(async () => window.review.compareNeutral());
    await saveImage(join(out, "neutral-native.png"), comparison.image);
    await saveImage(join(out, "neutral-diff.png"), comparison.difference);
    const reference = await readFile(referenceFile);
    await writeFile(join(out, "neutral-reference.png"), reference);
    report.neutral_reconstruction = {
      status: comparison.status, exact: comparison.exact,
      reference_sha256: sha256(reference), width: comparison.width, height: comparison.height,
      capture: "first_frame_native_size_dpr_1_no_animation",
      comparison: "premultiplied_rgba_and_composited_black_white_8bit_units",
      metrics: comparison.metrics,
      reference_image: "neutral-reference.png", actual_image: "neutral-native.png", difference_image: "neutral-diff.png",
      difference_amplification: 8,
    };
  }
  const portraitCases = [
    ["neutral", {}], ["blink", { eyeLOpen: 0, eyeROpen: 0 }],
    ["blink_left", { eyeLOpen: 0 }], ["blink_right", { eyeROpen: 0 }],
    ["mouth_half", { mouthOpen: 0.5 }], ["mouth_open", { mouthOpen: 1 }],
    ["mouth_round", { mouthOpen: 1, vowel: -1 }], ["mouth_wide", { mouthOpen: 1, vowel: 1 }],
  ];
  const cases = args.profile !== "full" ? portraitCases : [
    ["neutral", {}],
    ["head_left", { angleX: -22 }], ["head_right", { angleX: 22 }],
    ["head_up", { angleY: 18 }], ["head_down", { angleY: -18 }],
    ["head_tilt", { angleZ: 20 }],
    ["gaze_left", { eyeBallX: -1 }], ["gaze_right", { eyeBallX: 1 }],
    ["gaze_up", { eyeBallY: 1 }], ["gaze_down", { eyeBallY: -1 }],
    ["blink", { eyeLOpen: 0, eyeROpen: 0 }],
    ["blink_left", { eyeLOpen: 0 }], ["blink_right", { eyeROpen: 0 }],
    ["brows_up", { browLY: 1, browRY: 1 }], ["brows_down", { browLY: -1, browRY: -1 }],
    ["mouth_half", { mouthOpen: 0.5 }], ["mouth_open", { mouthOpen: 1 }],
    ["mouth_round", { mouthOpen: 1, vowel: -1 }], ["mouth_wide", { mouthOpen: 1, vowel: 1 }],
    ["body_lean", { bodyX: 8, bodyZ: 8 }], ["breath", { breath: 1 }],
    ...info.expressions.map((expression, index) => [`expression_${index + 1}`, {}, expression.id]),
  ];
  const references = { mouth_open: ["mouth_half"], mouth_round: ["mouth_open"], mouth_wide: ["mouth_open", "mouth_round"], blink_right: ["blink_left"] };
  if (["gaze", "puppet"].includes(args.profile)) {
    cases.push(
      ["gaze_left", {eyeBallX:-1}], ["gaze_right", {eyeBallX:1}],
      ["gaze_up", {eyeBallY:1}], ["gaze_down", {eyeBallY:-1}],
      ["gaze_diagonal_up", {eyeBallX:-0.8,eyeBallY:0.7}],
      ["gaze_diagonal_down", {eyeBallX:0.8,eyeBallY:-0.65}],
      ["half_blink", {eyeLOpen:0.5,eyeROpen:0.5}],
      ["half_blink_gaze_left", {eyeLOpen:0.5,eyeROpen:0.5,eyeBallX:-1}],
      ["half_blink_gaze_right", {eyeLOpen:0.5,eyeROpen:0.5,eyeBallX:1}],
      ["half_blink_gaze_up", {eyeLOpen:0.5,eyeROpen:0.5,eyeBallY:1}],
      ["half_blink_gaze_down", {eyeLOpen:0.5,eyeROpen:0.5,eyeBallY:-1}],
      ["closed_gaze_left", {eyeLOpen:0,eyeROpen:0,eyeBallX:-1}],
    );
    for (const [id] of cases) if (id.startsWith("half_blink_gaze_")) references[id] = ["half_blink"];
    references.closed_gaze_left = ["blink"];
  }
  if (args.profile === "puppet") {
    cases.push(
      ["head_left",{angleX:-14}],["head_right",{angleX:14}],
      ["head_up",{angleY:10}],["head_down",{angleY:-10}],
      ["head_roll_left",{angleZ:-6}],["head_roll_right",{angleZ:6}],
      ["head_left_half_blink",{angleX:-10,eyeLOpen:0.5,eyeROpen:0.5}],
      ["head_right_half_blink_gaze",{angleX:10,eyeLOpen:0.5,eyeROpen:0.5,eyeBallX:0.6}],
      ["head_nod_half_blink",{angleY:6,eyeLOpen:0.5,eyeROpen:0.5}],
      ["gentle_combined",{angleX:4,angleY:2,angleZ:1.5,eyeLOpen:0.8,eyeROpen:0.8,eyeBallX:0.4,breath:0.8}],
      ["breath",{breath:1}],
      ["body_left",{bodyX:-2}],["body_right",{bodyX:2}],
      ["body_lean_left",{bodyZ:-1}],["body_lean_right",{bodyZ:1}],
    );
    let corner=0;
    for(const angleX of [-14,14])for(const angleY of [-10,10])for(const angleZ of [-6,6])cases.push([
      `head_corner_${++corner}`,{angleX,angleY,angleZ},
    ]);
  }
  const expressionFrame = (id) => `expression_${info.expressions.findIndex((expression) => expression.id === id) + 1}`;
  if (args.profile === "full" && info.expressions.some((expression) => expression.id === "smile")) {
    cases.push(["smile_mouth_open", { mouthOpen: 1 }, "smile"], ["smile_mouth_round", { mouthOpen: 1, vowel: -1 }, "smile"], ["smile_mouth_wide", { mouthOpen: 1, vowel: 1 }, "smile"]);
    references.smile_mouth_open = [expressionFrame("smile")];
    references.smile_mouth_round = ["smile_mouth_open"];
    references.smile_mouth_wide = ["smile_mouth_open", "smile_mouth_round"];
  }
  if (args.profile === "full" && info.expressions.some((expression) => expression.id === "shy")) {
    cases.push(["shy_blink", { eyeLOpen: 0, eyeROpen: 0 }, "shy"], ["shy_gaze", { eyeBallX: 1 }, "shy"]);
    references.shy_blink = [expressionFrame("shy")];
    references.shy_gaze = [expressionFrame("shy")];
  }
  for (const [id, params, expression = null] of cases) {
    const frame = await page.evaluate(async ({ id, params, expression, references }) => window.review.capture(id, params, expression, references), { id, params, expression, references: references[id] ?? [] });
    const file = `frames/${id}.png`;
    await saveImage(join(out, file), frame.image);
    const passed = frame.coverage > 0.005 && !frame.context_lost && frame.webgl_error === 0;
    const response_detected = id === "neutral" ? null : frame.changed > 0.00001 && frame.compared.every((reference) => reference.changed > 0.00001);
    report.checks.push({ id, params, expression, image: file, coverage: frame.coverage, changed_from_neutral: frame.changed, compared: frame.compared, response_detected, ...(frame.gaze ? {gaze:frame.gaze} : {}), ...(frame.puppet ? {puppet:frame.puppet} : {}), webgl_error: frame.webgl_error, passed });
  }
  if (["gaze", "puppet"].includes(args.profile)) {
    const check = (id) => report.checks.find((entry) => entry.id === id);
    const centroids = (id) => check(id).gaze.centroids;
    const directions = proofEyes.map((eye, index) => ({
      eye: eye.id,
      right_minus_left_pixels: centroids("gaze_right")[index].x - centroids("gaze_left")[index].x,
      down_minus_up_pixels: centroids("gaze_down")[index].y - centroids("gaze_up")[index].y,
    }));
    const clipping = report.checks.filter((entry) => entry.id.startsWith("gaze_") || entry.id.startsWith("half_blink_gaze_") || entry.id.startsWith("closed_gaze_")).map((entry) => ({
      id:entry.id,baseline:entry.gaze.baseline,
      changed_inside_eye_mask:entry.gaze.changed_inside_eye_mask,
      changed_outside_eye_mask:entry.gaze.changed_outside_eye_mask,
    }));
    const directional = directions.every((eye) => eye.right_minus_left_pixels > 0.5 && eye.down_minus_up_pixels > 0.5);
    const contained = clipping.every((entry) => entry.changed_outside_eye_mask === 0);
    const closedStill = check("closed_gaze_left").gaze.changed_inside_eye_mask === 0;
    const responds = clipping.filter((entry) => !entry.id.startsWith("closed_")).every((entry) => entry.changed_inside_eye_mask > 0);
    report.gaze_verification = {
      status: directional && contained && closedStill && responds ? "passed" : "failed",
      scope: "directional_darkness_centroids_and_changes_within_original_open_eye_mask_with_2px_margin",
      limitation: "The original open-eye mask is a conservative envelope, not an exact half-blink contour. These measurements do not assess visual quality.",
      eyes:proofEyes,directions,clipping,
      closed_gaze_unchanged:closedStill,
    };
  }
  if (args.profile === "puppet") {
    const headIds=["head_left","head_right","head_up","head_down","head_roll_left","head_roll_right"];
    const poses=report.checks.filter((entry)=>headIds.includes(entry.id));
    const all=report.checks.filter((entry)=>entry.puppet);
    const moves=poses.every((entry)=>entry.puppet.head_relative_to_lower_jacket>0.5);
    const anchored=poses.every((entry)=>entry.puppet.lower_jacket_translation&&Math.hypot(...entry.puppet.lower_jacket_translation)<=3);
    const covered=all.every((entry)=>entry.puppet.neck_core.samples>0&&entry.puppet.neck_core.transparent_pixels===0);
    const unclipped=all.every((entry)=>entry.puppet.head_border_pixels===0);
    report.puppet_verification={
      status:moves&&anchored&&covered&&unclipped?"passed":"failed",
      scope:"separate_head_body_contract_photometric_tracking_neck_core_alpha_and_head_canvas_bounds",
      limitation:"Photometric tracking estimates local motion. Neck coverage is sampled in a central probe; it cannot establish global seam quality or physical plausibility. Head-warp blink combinations require visual review.",
      structure:proofGeometry,head_motion_detected:moves,lower_jacket_anchored:anchored,neck_core_covered:covered,head_not_clipped:unclipped,
      tested_bounds:{angle_x:[-14,14],angle_y:[-10,10],angle_z:[-6,6],body_x:[-2,2],body_z:[-1,1],breath:[0,1],head_axis_corners:8,body_basis_cases:4,scope:"head_basis_and_head_corners_plus_body_basis_not_every_combined_head_body_corner"},
      poses:poses.map((entry)=>({id:entry.id,...entry.puppet})),
    };
  }
  const cards = report.checks.map((check) => `<figure><img src="${check.image}" alt="${escapeHtml(check.id)}"><figcaption>${escapeHtml(check.id.startsWith("expression_") ? check.expression : check.id)}</figcaption></figure>`).join("\n");
  const cardWidth = args.profile !== "full" ? 220 : 180;
  await writeFile(join(out, "contact-sheet.html"), `<!doctype html><html lang="en"><meta charset="utf-8"><title>Character playback samples</title><link rel="icon" href="data:,"><style>body{margin:20px;font:14px system-ui;background:#f8fafc;color:#172534}h1{font-size:22px}main{display:grid;grid-template-columns:repeat(${args.profile !== "full" ? 4 : 5},${cardWidth}px);gap:12px}figure{margin:0;border:1px solid #cbd5e1;border-radius:8px;overflow:hidden;background:repeating-conic-gradient(#eef1f5 0% 25%,#fff 0% 50%) 50% / 16px 16px}img{display:block;width:${cardWidth}px;height:${Math.round(cardWidth*4/3)}px;object-fit:contain}figcaption{padding:8px;background:white;text-align:center}</style><h1>${escapeHtml(info.name)}: technical playback samples</h1><p>Pixel differences demonstrate output changes. Naturalness and visual quality require human review.</p><main>${cards}</main></html>`);
  if (!args.no_video) {
    if (spawnSync("ffmpeg", ["-version"], { stdio: "ignore" }).status === 0) {
      const motion = join(out, "motion");
      await mkdir(motion, { recursive: true });
      const count = args.profile === "puppet" ? 360 : args.profile === "gaze" ? 288 : 72;
      const fps = ["gaze", "puppet"].includes(args.profile) ? 24 : 12;
      for (let index = 0; index < count; index++) {
        const image = await page.evaluate(async ({ index, count }) => window.review.motion(index, count), { index, count });
        await saveImage(join(motion, `${String(index).padStart(3, "0")}.png`), image);
        if (index === 0) await saveImage(join(out, "preview-poster.png"), image);
      }
      const result = spawnSync("ffmpeg", ["-v", "error", "-y", "-framerate", String(fps), "-i", join(motion, "%03d.png"), "-frames:v", String(count), "-c:v", "libx264", "-crf", "17", "-pix_fmt", "yuv420p", "-movflags", "+faststart", join(out, "preview.mp4")], { encoding: "utf8" });
      report.video = result.status === 0 ? { status: "created", file: "preview.mp4", poster: "preview-poster.png", frames: count, fps, duration_seconds:count/fps, ...(["gaze", "puppet"].includes(args.profile) ? {width:720,height:960,framing:args.profile==="puppet"?"head_shoulders_and_lower_jacket_anchor":"head_and_shoulders",sequence:args.profile==="puppet"?"yaw_nod_roll_then_gentle_head_breath_gaze_blinks":"neutral_left_right_up_down_two_diagonals_blink_speech",background:"neutral_gradient"} : {}) } : { status: "failed", error: sanitize(result.stderr) };
    } else report.video = { status: "skipped", reason: "FFmpeg is not on PATH." };
  }
  await page.evaluate(() => window.review.dispose());
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto(`${origin}/contact-sheet.html`);
  await page.evaluate(async () => Promise.all(Array.from(document.images, (image) => image.decode())));
  await page.screenshot({ path: join(out, "contact-sheet.png"), fullPage: true });
  report.status = report.checks.every((check) => check.passed) && report.gaze_verification?.status !== "failed" && report.puppet_verification?.status !== "failed" && !report.console_errors.length && !report.page_errors.length && !report.failed_requests.length && !report.blocked_external_requests && report.video.status !== "failed" ? "passed" : "failed";
} catch (error) {
  report.error = sanitize(error.message);
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
  await writeFile(join(out, "report.json"), JSON.stringify(report, null, 2) + "\n");
}
console.log(`Technical playback ${report.status}: ${report.checks.filter((check) => check.passed).length}/${report.checks.length} cases; ${report.console_errors.length} console errors; ${report.page_errors.length} page errors; video ${report.video.status}; neutral reconstruction ${report.neutral_reconstruction.status}; naturalness unassessed.`);
if (report.error) console.error(report.error);
if (report.status !== "passed") process.exitCode = 1;
