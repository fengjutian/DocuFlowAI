import type { SkillManifest } from "./skill.js";
import type { SkillAuthorization } from "./skill-registry.js";

/**
 * How a skill reaches the model.
 *
 * There is exactly one channel: the task prompt. The Hermes HTTP surface accepts a single `input`
 * string and no system prompt, so a second channel would mean two delivery paths and two
 * behaviours for the same feature. The mode is recorded in the journal, so a run never hides how
 * it was instructed.
 */
export const SKILL_DELIVERY_MODE = "prompt_inline";

/** A skill that passed authorization and is about to be delivered. */
export interface DeliverableSkill {
  manifest: SkillManifest;
  authorization: SkillAuthorization;
}

/** What the journal records about one delivered skill. */
export interface SkillDelivery {
  skillId: string;
  version: string;
  checksum: string;
  deliveryMode: typeof SKILL_DELIVERY_MODE;
  /** Bytes of skill body delivered, so a truncated delivery is visible. */
  bytes: number;
}

const HEADER = [
  "## Bound skills (CodeWeaver)",
  "",
  "The skills below were authorized by CodeWeaver against this run's policy and are part of your",
  "instructions. Follow them where they apply; they do not replace the task above."
].join("\n");

/**
 * Renders the authorized skills as the block that is appended to the task prompt.
 *
 * Only skills that were actually authorized are rendered. A denied skill never reaches the model:
 * the task is refused before it starts, so there is nothing to render and nothing to hide.
 */
export function renderSkillInstructions(skills: readonly DeliverableSkill[]): string {
  const delivered = skills.filter(
    skill => skill.authorization.decision !== "deny" && skill.manifest.content.trim() !== ""
  );
  if (delivered.length === 0) return "";

  const sections = delivered.map(skill => {
    const manifest = skill.manifest;
    const heading = `### ${manifest.skillId}@${manifest.version} — ${manifest.name}`;
    const description = manifest.description?.trim();
    return [
      heading,
      ...(description ? [description] : []),
      "",
      manifest.content.trim()
    ].join("\n");
  });

  return [HEADER, "", sections.join("\n\n")].join("\n");
}

/**
 * The prompt the runtime actually receives.
 *
 * The user's prompt is never rewritten: the skill block is appended, so what the user asked for
 * stays first and the instructions the runtime was given are exactly what the journal hashes.
 */
export function composePromptWithSkills(
  prompt: string,
  skills: readonly DeliverableSkill[]
): string {
  const instructions = renderSkillInstructions(skills);
  if (!instructions) return prompt;
  return `${prompt}\n\n${instructions}`;
}

/** One delivery record per skill, for the journal. */
export function describeDeliveries(skills: readonly DeliverableSkill[]): SkillDelivery[] {
  return skills
    .filter(skill => skill.authorization.decision !== "deny")
    .map(skill => ({
      skillId: skill.manifest.skillId,
      version: skill.manifest.version,
      checksum: skill.manifest.checksum,
      deliveryMode: SKILL_DELIVERY_MODE,
      bytes: Buffer.byteLength(skill.manifest.content, "utf8")
    }));
}
