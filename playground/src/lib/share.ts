import { gzip, ungzip } from "pako";
import type { Level } from "./constants";

const SHARE_SCHEMA_VERSION = "1";
const SHARE_HASH_PREFIX = "state=";
const MAX_ENCODED_STATE_LENGTH = 200_000;
const MAX_SOURCE_LENGTH = 1_000_000;
export const SHARE_LIMIT_MESSAGE = "Input is too large to share";

export interface PlaygroundShareState {
  source: string;
  level: Level;
  formatter: boolean;
  vueSfc: boolean;
  version: string;
}

export function readShareState(hash = window.location.hash): PlaygroundShareState | null {
  if (!hash || hash === "#") {
    return null;
  }

  const rawState = decodeHashState(hash.slice(1));
  if (rawState === null) {
    return null;
  }

  if (!rawState.startsWith(SHARE_HASH_PREFIX)) {
    return null;
  }

  const stateValue = rawState.slice(SHARE_HASH_PREFIX.length);
  const separatorIndex = stateValue.indexOf("|");
  if (separatorIndex === -1) {
    return null;
  }

  const schema = stateValue.slice(0, separatorIndex);
  const encodedState = stateValue.slice(separatorIndex + 1);
  if (
    schema !== SHARE_SCHEMA_VERSION ||
    !encodedState ||
    encodedState.length > MAX_ENCODED_STATE_LENGTH
  ) {
    return null;
  }

  try {
    const json = ungzip(decodeBase64Url(encodedState), { to: "string" });
    const parsed = JSON.parse(json) as Partial<PlaygroundShareState>;
    const formatter = normalizeFormatter(parsed.formatter);
    const vueSfc = normalizeVueSfc(parsed.vueSfc);
    if (
      typeof parsed.source !== "string" ||
      !isLevel(parsed.level) ||
      formatter === null ||
      vueSfc === null ||
      typeof parsed.version !== "string"
    ) {
      return null;
    }

    if (parsed.source.length > MAX_SOURCE_LENGTH) {
      return null;
    }

    return {
      source: parsed.source,
      level: parsed.level,
      formatter,
      vueSfc,
      version: parsed.version,
    };
  } catch {
    return null;
  }
}

export function createShareUrl(state: PlaygroundShareState, href = window.location.href): string {
  if (state.source.length > MAX_SOURCE_LENGTH) {
    throw new Error(SHARE_LIMIT_MESSAGE);
  }

  const url = new URL(href);
  const encodedState = encodeBase64Url(gzip(JSON.stringify(state)));
  if (encodedState.length > MAX_ENCODED_STATE_LENGTH) {
    throw new Error(SHARE_LIMIT_MESSAGE);
  }

  url.hash = `${SHARE_HASH_PREFIX}${SHARE_SCHEMA_VERSION}|${encodedState}`;
  return url.toString();
}

function isLevel(value: unknown): value is Level {
  return value === "minimal" || value === "standard" || value === "aggressive";
}

function normalizeFormatter(value: unknown): boolean | null {
  if (value === undefined) {
    return true;
  }
  if (typeof value === "boolean") {
    return value;
  }
  return null;
}

function normalizeVueSfc(value: unknown): boolean | null {
  if (value === undefined) {
    return false;
  }
  if (typeof value === "boolean") {
    return value;
  }
  return null;
}

function decodeHashState(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.slice(i, i + 0x8000));
  }

  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=/g, "");
}

function decodeBase64Url(value: string): Uint8Array {
  const padded = value.replace(/-/g, "+").replace(/_/g, "/").padEnd(
    Math.ceil(value.length / 4) * 4,
    "="
  );
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }

  return bytes;
}
