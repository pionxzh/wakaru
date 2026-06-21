const VLQ_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

function decodeVLQ(encoded: string): number[] {
  const values: number[] = [];
  let shift = 0;
  let value = 0;
  for (const ch of encoded) {
    const digit = VLQ_CHARS.indexOf(ch);
    if (digit < 0) continue;
    value += (digit & 31) << shift;
    if (digit & 32) {
      shift += 5;
    } else {
      values.push(value & 1 ? -(value >> 1) : value >> 1);
      value = 0;
      shift = 0;
    }
  }
  return values;
}

interface MappingToken {
  genLine: number;
  genCol: number;
  srcLine: number;
  srcCol: number;
}

function parseTokens(mappingsStr: string): MappingToken[] {
  const tokens: MappingToken[] = [];
  let gCol = 0;
  let sLine = 0;
  let sCol = 0;
  let sIdx = 0;
  mappingsStr.split(";").forEach((line, genLine) => {
    gCol = 0;
    if (!line) return;
    line.split(",").forEach((seg) => {
      if (!seg) return;
      const v = decodeVLQ(seg);
      gCol += v[0];
      if (v.length >= 4) {
        sIdx += v[1];
        sLine += v[2];
        sCol += v[3];
        tokens.push({ genLine, genCol: gCol, srcLine: sLine, srcCol: sCol });
      }
    });
  });
  return tokens;
}

// 16 bold, high-saturation line colors
const LINE_COLORS = [
  "rgba(255,120,80,{a})",
  "rgba(80,165,255,{a})",
  "rgba(80,210,110,{a})",
  "rgba(190,110,255,{a})",
  "rgba(255,190,50,{a})",
  "rgba(60,210,220,{a})",
  "rgba(255,105,180,{a})",
  "rgba(110,220,140,{a})",
  "rgba(240,180,60,{a})",
  "rgba(140,120,255,{a})",
  "rgba(255,100,100,{a})",
  "rgba(50,200,200,{a})",
  "rgba(220,170,80,{a})",
  "rgba(100,180,255,{a})",
  "rgba(160,230,80,{a})",
  "rgba(210,120,240,{a})",
];

export const LINE_COLORS_RGB = LINE_COLORS.map(
  (tpl) => tpl.replace("rgba(", "").replace(",{a})", "")
);

export function lineColorClass(lineIndex: number): string {
  return `mapping-line-${lineIndex % LINE_COLORS.length}`;
}

export function lineColorActiveClass(lineIndex: number): string {
  return `mapping-line-${lineIndex % LINE_COLORS.length}-active`;
}

export function generateMappingCSS(): string {
  return LINE_COLORS.map((tpl, i) => {
    const bg = tpl.replace("{a}", "0.22");
    const activeBg = tpl.replace("{a}", "0.5");
    return `.mapping-line-${i} { background: ${bg} !important; }\n.mapping-line-${i}-active { background: ${activeBg} !important; }`;
  }).join("\n");
}

export interface MappingRegion {
  genLine: number;
  genStartCol: number;
  genEndCol: number;
  srcLine: number;
  srcStartCol: number;
  srcEndCol: number;
  colorIndex: number;
}

export interface MappingData {
  regions: MappingRegion[];
  coveragePct: number;
}

export function parseMappings(
  sourceMapJson: string,
  outputCode: string
): MappingData {
  const sm = JSON.parse(sourceMapJson);
  const tokens = parseTokens(sm.mappings);
  const genLines = outputCode.split("\n");
  const srcLines = (sm.sourcesContent?.[0] ?? "").split("\n");

  // Build a sorted list of all unique source positions per source line,
  // so we can compute source-side ranges correctly.
  const srcPositions = new Map<number, number[]>();
  for (const t of tokens) {
    let cols = srcPositions.get(t.srcLine);
    if (!cols) {
      cols = [];
      srcPositions.set(t.srcLine, cols);
    }
    if (!cols.includes(t.srcCol)) cols.push(t.srcCol);
  }
  for (const cols of srcPositions.values()) cols.sort((a, b) => a - b);

  function srcEndFor(srcLine: number, srcCol: number): number {
    const cols = srcPositions.get(srcLine);
    if (!cols) return srcCol;
    const idx = cols.indexOf(srcCol);
    if (idx >= 0 && idx + 1 < cols.length) return cols[idx + 1];
    return (srcLines[srcLine] ?? "").length;
  }

  const regions: MappingRegion[] = [];
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i];
    const nextOnLine = tokens.find(
      (t2, j) => j > i && t2.genLine === t.genLine
    );
    const lineLen = (genLines[t.genLine] || "").length;
    const genEnd = nextOnLine ? nextOnLine.genCol : lineLen;
    if (genEnd <= t.genCol) continue;

    regions.push({
      genLine: t.genLine,
      genStartCol: t.genCol,
      genEndCol: genEnd,
      srcLine: t.srcLine,
      srcStartCol: t.srcCol,
      srcEndCol: srcEndFor(t.srcLine, t.srcCol),
      colorIndex: t.genLine % LINE_COLORS.length,
    });
  }

  // Coverage: % of non-whitespace output characters that are mapped
  let mapped = 0;
  let total = 0;
  const genCharMap = genLines.map((l) => new Uint8Array(l.length));
  for (const r of regions) {
    const line = genCharMap[r.genLine];
    if (!line) continue;
    for (let c = r.genStartCol; c < r.genEndCol && c < line.length; c++) {
      line[c] = 1;
    }
  }
  genLines.forEach((line, li) => {
    for (let c = 0; c < line.length; c++) {
      if (line[c] !== " " && line[c] !== "\t") {
        total++;
        if (genCharMap[li][c]) mapped++;
      }
    }
  });

  return {
    regions,
    coveragePct: total ? Math.round((100 * mapped) / total) : 0,
  };
}
