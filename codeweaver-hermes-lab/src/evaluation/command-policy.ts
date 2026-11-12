import { createHash } from "node:crypto";

/**
 * The command policy of the Evaluation Engine.
 *
 * An acceptance check is a real process, so it can do what a process can do. The agent may narrow
 * the set of checks but never introduce one, so the danger arrives with the task's declaration —
 * and the product has to answer it: this command does not run just because somebody typed it.
 *
 * Two things follow from the checklist (P0-012, "根据策略拦截或审批") and from the rule that an
 * approval may only ever authorize one specific operation:
 *
 *  1. A command on the dangerous list is refused unless a human approved *that exact command*.
 *     The approval carries a fingerprint of the command, so an approval for one command cannot be
 *     replayed for another one.
 *  2. The refusal is the product's own decision. A runtime's own approval dialog is not the policy
 *     engine, and an approval never widens anything beyond the one operation it names.
 *
 * What this is not: a sandbox. The classifier reads the command line it is given, so a check that
 * hides its work inside a script file is beyond it — that is what the runtime isolation items
 * (P0-016, P0-017) are for. What it does promise is that the dangerous commands people actually
 * type are refused unless somebody said yes to that exact line.
 */

/** A human decision about one exact command. */
export interface CommandApproval {
  approvedBy: string;
  reason: string;
  approvedAt: string;
  /** Fingerprint of the command this approval was given for. */
  commandFingerprint: string;
}

export interface CommandClassification {
  /** True when the command must not run without an explicit approval. */
  dangerous: boolean;
  /** Why it is dangerous, in the words of the rule that caught it. */
  reasons: string[];
  /** Identity of this exact command; an approval is bound to it. */
  fingerprint: string;
}

/** Programs that destroy data. */
const DESTRUCTIVE_PROGRAMS = new Set([
  "rm",
  "rmdir",
  "del",
  "rd",
  "shred",
  "srm",
  "wipe",
  "erase",
  "unlink",
  "mkfs",
  "format",
  "diskpart",
  "dd",
  "fdisk",
  "parted",
  "truncate",
  "remove-item",
  "clear-content",
  "del-tree"
]);

/** Programs that change the system's permissions or identity. */
const PRIVILEGE_PROGRAMS = new Set([
  "sudo",
  "su",
  "doas",
  "runas",
  "pkexec",
  "chown",
  "setfacl",
  "icacls",
  "takeown"
]);

/** Programs that stop or kill things. */
const PROCESS_CONTROL_PROGRAMS = new Set([
  "kill",
  "killall",
  "pkill",
  "taskkill",
  "stop-process",
  "shutdown",
  "reboot",
  "halt",
  "poweroff",
  "systemctl",
  "service"
]);

/** Programs that run a command line the policy cannot read. */
const SHELL_PROGRAMS = new Set(["cmd", "powershell", "pwsh", "sh", "bash", "zsh", "fish", "ksh"]);

/** The flag a shell takes the command line after. */
const SHELL_COMMAND_FLAGS = new Set(["/c", "/k", "-c", "-command", "--command"]);

/** Wrapper programs whose sub-commands carry the danger. */
const SUBCOMMAND_RULES: Array<{ program: string; commands: string[]; reason: string }> = [
  { program: "git", commands: ["push", "clean", "filter-branch"], reason: "rewrites or publishes history" },
  { program: "npm", commands: ["publish", "unpublish", "login", "token"], reason: "publishes packages or credentials" },
  { program: "pnpm", commands: ["publish", "login"], reason: "publishes packages or credentials" },
  { program: "yarn", commands: ["publish", "login"], reason: "publishes packages or credentials" },
  { program: "pip", commands: ["uninstall"], reason: "removes installed packages" },
  { program: "twine", commands: ["upload"], reason: "publishes packages" },
  { program: "cargo", commands: ["publish", "login"], reason: "publishes packages or credentials" },
  { program: "gem", commands: ["push"], reason: "publishes packages" },
  { program: "docker", commands: ["rm", "rmi", "system", "volume"], reason: "destroys containers, images or volumes" },
  { program: "kubectl", commands: ["delete", "apply", "exec"], reason: "changes or runs things in a cluster" },
  { program: "aws", commands: ["configure", "s3"], reason: "changes cloud credentials or data" },
  { program: "az", commands: ["login", "account"], reason: "changes cloud credentials" },
  { program: "gcloud", commands: ["auth", "projects"], reason: "changes cloud credentials" }
];

/** Sub-commands that are only dangerous with a force flag. */
const FORCE_SUBCOMMANDS: Array<{ program: string; command: string; reason: string }> = [
  { program: "git", command: "clean", reason: "deletes untracked files" },
  { program: "docker", command: "system", reason: "deletes containers, images or volumes" }
];

const FORCE_FLAGS = new Set(["-f", "-rf", "-fr", "--force", "-fd", "-fdx", "-x", "-d", "/f", "--dangerous"]);

/**
 * Classifies a command line.
 *
 * The command is judged as it was written: the program, its arguments, and — when the program is a
 * shell — the command line hidden inside it.
 */
export function classifyCommand(command: string, args: readonly string[] = []): CommandClassification {
  const reasons: string[] = [];
  const program = programName(command);
  const argv = [command, ...args];
  const tokens = argv.flatMap(token => token.split(/\s+/)).filter(token => token !== "");

  if (SHELL_PROGRAMS.has(program)) {
    reasons.push(`'${program}' runs a command line the policy cannot read`);
  }

  // The program itself.
  if (DESTRUCTIVE_PROGRAMS.has(program)) {
    reasons.push(`'${program}' destroys data`);
  }
  if (PRIVILEGE_PROGRAMS.has(program)) {
    reasons.push(`'${program}' changes permissions or identity`);
  }
  if (PROCESS_CONTROL_PROGRAMS.has(program)) {
    reasons.push(`'${program}' stops or kills running things`);
  }
  if (program === "chmod" && (tokens.includes("777") || tokens.includes("-R") || tokens.includes("--recursive"))) {
    reasons.push("'chmod' opens permissions up or applies them recursively");
  }

  // What a shell was told to run.
  const inner = innerCommandTokens(program, tokens);
  for (const token of inner) {
    const name = programName(token);
    if (DESTRUCTIVE_PROGRAMS.has(name)) reasons.push(`'${name}' destroys data`);
    if (PRIVILEGE_PROGRAMS.has(name)) reasons.push(`'${name}' changes permissions or identity`);
    if (PROCESS_CONTROL_PROGRAMS.has(name)) reasons.push(`'${name}' stops or kills running things`);
  }

  // Sub-commands of wrapper programs.
  const sub = tokens.map(programName);
  for (const rule of SUBCOMMAND_RULES) {
    if (program !== rule.program) continue;
    if (sub.includes(rule.commands[0]!)) {
      reasons.push(`'${rule.program} ${rule.commands[0]}' ${rule.reason}`);
    }
  }
  for (const rule of FORCE_SUBCOMMANDS) {
    if (program !== rule.program) continue;
    if (!sub.includes(rule.command)) continue;
    if (tokens.some(token => FORCE_FLAGS.has(token.toLowerCase()))) {
      reasons.push(`'${rule.program} ${rule.command}' ${rule.reason}`);
    }
  }

  // A download piped into a shell runs code nobody reviewed.
  if (isDownload(program) && tokens.some(token => token === "|" || token === "| sh" || token === "| bash")) {
    reasons.push("the command downloads and pipes into a shell");
  }
  if (tokens.some((token, index) => token === "|" && SHELL_PROGRAMS.has(programName(tokens[index + 1] ?? "")))) {
    reasons.push("the command pipes into a shell");
  }

  return {
    dangerous: reasons.length > 0,
    reasons: [...new Set(reasons)],
    fingerprint: commandFingerprint(command, args)
  };
}

/** Identity of one exact command: the same line always has the same fingerprint. */
export function commandFingerprint(command: string, args: readonly string[] = []): string {
  return createHash("sha256").update(JSON.stringify([command, [...args]]), "utf8").digest("hex");
}

/**
 * Whether an approval covers exactly this command.
 *
 * The approval names the fingerprint it was given for, so it cannot be moved to another command —
 * approving "remove the build directory" is not approving "remove everything".
 */
export function approvalCovers(
  approval: CommandApproval | undefined,
  command: string,
  args: readonly string[] = []
): boolean {
  if (!approval) return false;
  return approval.commandFingerprint === commandFingerprint(command, args);
}

/** The program name, lowercased and without its extension. */
function programName(token: string): string {
  const base = token.split(/[\\/]/).pop() ?? token;
  const withoutExtension = base.replace(/\.(exe|cmd|bat|ps1|sh)$/i, "");
  return withoutExtension.toLowerCase();
}

/** The command line a shell was told to run, as tokens. */
function innerCommandTokens(program: string, tokens: readonly string[]): string[] {
  if (!SHELL_PROGRAMS.has(program)) return [];
  const flagIndex = tokens.findIndex(token => SHELL_COMMAND_FLAGS.has(token.toLowerCase()));
  if (flagIndex < 0) return [];
  return tokens.slice(flagIndex + 1).flatMap(token => token.split(/\s+/)).filter(token => token !== "");
}

function isDownload(program: string): boolean {
  return program === "curl" || program === "wget" || program === "invoke-webrequest";
}
