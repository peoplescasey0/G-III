/**
 * Paper-plane presentation and payload rules for the Desk throw.
 *
 * The throw vector itself (direction, force, arc, landing target) lives in
 * `throw-vector.ts` and is unchanged by this module. This file only decides how
 * the plane reads as it leaves the hand and what note it carries, so both can
 * be tested without a WebGL context.
 */

/** How long the sheet takes to read as a folded dart after leaving the hand. */
export const PLANE_FOLD_MS = 220;
/** The guest API rejects notes longer than this; the dialog's textarea matches. */
export const PLANE_NOTE_LIMIT = 160;

export type FoldPose = {
  /** Radians each wing lifts away from the flat sheet. */
  dihedral: number;
  /** Radians the keel still hangs below the sheet; reaches zero when folded. */
  keelDrop: number;
  /** The sheet starts larger than the finished dart. */
  sheetScale: number;
};

/**
 * A flat sheet becomes a dart: wings lift, the keel tucks in, and the sheet
 * shrinks to the finished plane. Progress 0 is the flat sheet in the hand,
 * progress 1 is the dart in flight.
 */
export function foldPose(progress: number): FoldPose {
  const t = clamp01(progress);
  const eased = t * t * (3 - 2 * t);
  return {
    dihedral: eased * 0.38,
    keelDrop: (1 - eased) * 0.55,
    sheetScale: 1.28 - eased * 0.28,
  };
}

/**
 * The note the plane carries. A draft typed into the send dialog travels with
 * the plane verbatim; without a draft the plane still carries a short, honest
 * line naming where it came from and where it is going. The result is never
 * empty because the guest API rejects empty messages, and never longer than
 * the limit that API enforces.
 */
export function carriedNote(draft: string, fromName: string, toName: string): string {
  const trimmed = draft.replace(/\s+/g, " ").trim();
  if (trimmed) return trimmed.slice(0, PLANE_NOTE_LIMIT);
  const sender = fromName.trim() || "a card";
  const recipient = toName.trim();
  const fallback = recipient
    ? `✈ a paper plane from ${sender} for ${recipient}`
    : `✈ a paper plane from ${sender}`;
  return fallback.slice(0, PLANE_NOTE_LIMIT);
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}
