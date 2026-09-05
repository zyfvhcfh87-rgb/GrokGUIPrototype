export const MAX_EXTERNAL_URL_CHARS = 2048;

export type ExternalUrlFailureReason =
  | "empty"
  | "too_long"
  | "invalid"
  | "unsupported_scheme"
  | "credentials"
  | "missing_host";

export type ExternalUrlValidation =
  | { ok: true; href: string }
  | { ok: false; reason: ExternalUrlFailureReason };

export function validateExternalUrl(value: string): ExternalUrlValidation {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return { ok: false, reason: "empty" };
  }
  if (trimmed.length > MAX_EXTERNAL_URL_CHARS) {
    return { ok: false, reason: "too_long" };
  }
  if (trimmed.split("").some((character) => isForbiddenUrlCharacter(character))) {
    return { ok: false, reason: "invalid" };
  }

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return { ok: false, reason: "invalid" };
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { ok: false, reason: "unsupported_scheme" };
  }
  if (parsed.username !== "" || parsed.password !== "") {
    return { ok: false, reason: "credentials" };
  }
  if (parsed.hostname === "") {
    return { ok: false, reason: "missing_host" };
  }
  return { ok: true, href: parsed.href };
}

function isForbiddenUrlCharacter(character: string): boolean {
  const code = character.charCodeAt(0);
  return (
    character === "\\" ||
    character === " " ||
    character === "\t" ||
    (code >= 0 && code <= 31) ||
    code === 127
  );
}
