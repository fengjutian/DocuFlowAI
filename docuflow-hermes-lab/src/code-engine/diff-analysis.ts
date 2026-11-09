import { createHash } from "node:crypto";

/**
 * Unified-diff analysis for the Code Engine (design 7.3 step "生成 Diff / 风险分析").
 *
 * The parser is deliberately strict: anything it cannot attribute to a concrete relative path is a
 * blocking risk, because an unattributable patch is exactly how a path escape would slip through.
 */
export type RiskCode =
  | "path_escape"
  | "absolute_path"
  | "git_metadata"
  | "secret_file"
  | "dependency_change"
  | "migration"
  | "deletion"
  | "binary_patch"
  | "large_patch"
  | "no_files";

export interface ChangeRisk {
  code: RiskCode;
  message: string;
  /** Blocking risks reject the patch outright; the rest only demand approval. */
  blocked: boolean;
}

export interface ChangeProposal {
  patch: string;
  diffHash: string;
  files: string[];
  risks: ChangeRisk[];
  requiresApproval: boolean;
  blocked: boolean;
  summary: string;
}

export interface ProposeOptions {
  /** Patches larger than this need approval regardless of content. */
  maxPatchChars?: number;
}

const SECRET_PATTERNS: RegExp[] = [
  /(^|\/)\.env(\.|$)/i,
  /(^|\/)secrets?(\/|$)/i,
  /(^|\/)credentials?(\/|$)/i,
  /\.pem$/i,
  /\.key$/i,
  /(^|\/)id_(rsa|ed25519|ecdsa)(\.pub)?$/i,
  /(^|\/)\.npmrc$/i,
  /(^|\/)\.pypirc$/i
];

const DEPENDENCY_FILES = new Set([
  "package.json",
  "package-lock.json",
  "pnpm-lock.yaml",
  "yarn.lock",
  "npm-shrinkwrap.json",
  "requirements.txt",
  "pyproject.toml",
  "poetry.lock",
  "pipfile",
  "pipfile.lock",
  "go.mod",
  "go.sum",
  "pom.xml",
  "build.gradle",
  "build.gradle.kts",
  "cargo.toml",
  "cargo.lock",
  "composer.json",
  "composer.lock"
]);

const MIGRATION_PATTERNS: RegExp[] = [
  /(^|\/)migrations?\//i,
  /(^|\/)db\/migrate\//i,
  /(^|\/)alembic\//i,
  /(^|\/)flyway\//i
];

export function proposeChange(patch: string, options: ProposeOptions = {}): ChangeProposal {
  const files = extractPatchFiles(patch);
  const risks: ChangeRisk[] = [];

  if (files.length === 0) {
    risks.push({ code: "no_files", message: "Patch does not touch any file", blocked: true });
  }

  for (const file of files) {
    const pathRisk = inspectPath(file);
    if (pathRisk) risks.push(pathRisk);
  }

  for (const file of files) {
    const base = file.split("/").pop() ?? file;
    if (SECRET_PATTERNS.some(pattern => pattern.test(file))) {
      risks.push({ code: "secret_file", message: `'${file}' looks like a secret-bearing file`, blocked: false });
    }
    if (DEPENDENCY_FILES.has(base.toLowerCase())) {
      risks.push({ code: "dependency_change", message: `'${file}' changes dependencies`, blocked: false });
    }
    if (MIGRATION_PATTERNS.some(pattern => pattern.test(file))) {
      risks.push({ code: "migration", message: `'${file}' is a database migration`, blocked: false });
    }
  }

  if (/^deleted file mode /m.test(patch)) {
    risks.push({ code: "deletion", message: "Patch deletes at least one file", blocked: false });
  }
  if (/^Binary files .* differ$/m.test(patch) || /^GIT binary patch$/m.test(patch)) {
    risks.push({ code: "binary_patch", message: "Patch contains binary content", blocked: false });
  }
  const maxChars = options.maxPatchChars ?? 200_000;
  if (patch.length > maxChars) {
    risks.push({ code: "large_patch", message: `Patch is ${patch.length} characters (limit ${maxChars})`, blocked: false });
  }

  const deduped = dedupeRisks(risks);
  return {
    patch,
    diffHash: hashPatch(patch),
    files: [...new Set(files)].sort(),
    risks: deduped,
    requiresApproval: deduped.length > 0,
    blocked: deduped.some(risk => risk.blocked),
    summary: `${files.length} file(s), ${deduped.length} risk(s)`
  };
}

export function hashPatch(patch: string): string {
  return createHash("sha256").update(patch, "utf8").digest("hex");
}

/** Paths mentioned by a patch, normalized and de-duplicated. */
export function extractPatchFiles(patch: string): string[] {
  const files = new Set<string>();
  for (const rawLine of patch.split(/\r?\n/)) {
    const line = rawLine;
    if (line.startsWith("diff --git ")) {
      const match = /^diff --git (.+) (.+)$/.exec(line);
      if (match) {
        addPath(files, stripPrefix(unquote(match[1]!)));
        addPath(files, stripPrefix(unquote(match[2]!)));
      }
      continue;
    }
    for (const prefix of ["--- ", "+++ "]) {
      if (!line.startsWith(prefix)) continue;
      const candidate = line.slice(prefix.length).trim();
      if (candidate === "/dev/null") continue;
      addPath(files, stripPrefix(unquote(candidate.split("\t")[0]!)));
    }
    const rename = /^(rename|copy) (from|to) (.+)$/.exec(line);
    if (rename) {
      addPath(files, unquote(rename[3]!));
    }
  }
  return [...files];
}

function addPath(files: Set<string>, path: string): void {
  if (path) files.add(path.replace(/^\.\//, ""));
}

function stripPrefix(path: string): string {
  return path.replace(/^[ab]\//, "");
}

function inspectPath(file: string): ChangeRisk | null {
  if (file.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(file)) {
    return { code: "absolute_path", message: `'${file}' is an absolute path`, blocked: true };
  }
  const segments = file.split(/[\\/]/);
  if (segments.includes("..")) {
    return { code: "path_escape", message: `'${file}' escapes the worktree with '..'`, blocked: true };
  }
  if (segments[0] === ".git" || segments.includes(".git")) {
    return { code: "git_metadata", message: `'${file}' writes into git metadata`, blocked: true };
  }
  return null;
}

function dedupeRisks(risks: ChangeRisk[]): ChangeRisk[] {
  const seen = new Set<string>();
  return risks.filter(risk => {
    const key = `${risk.code}:${risk.message}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Git quotes unusual paths as C strings; unescape the subset that appears in patches. */
function unquote(value: string): string {
  const trimmed = value.trim();
  if (!trimmed.startsWith("\"") || !trimmed.endsWith("\"")) return trimmed;
  return trimmed
    .slice(1, -1)
    .replace(/\\"/g, "\"")
    .replace(/\\t/g, "\t")
    .replace(/\\n/g, "\n")
    .replace(/\\\\/g, "\\");
}
