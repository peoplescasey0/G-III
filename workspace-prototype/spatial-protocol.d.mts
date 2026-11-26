export type SpatialVec3 = { x: number; y: number; z: number };
export type SpatialQuat = { x: number; y: number; z: number; w: number };

export type SpatialPose = {
  position: SpatialVec3;
  rotation: SpatialQuat;
  scale: SpatialVec3;
};

export type SpatialSource = "studio" | "hypervisor" | "kernel";

export type SpatialCardState = {
  id: string;
  pose: SpatialPose;
  width: number;
  height: number;
  active: boolean;
  focused: boolean;
  source: SpatialSource;
};

export type SpatialCamera = {
  viewport: { width: number; height: number };
  fovY: number;
  position: SpatialVec3;
  rotation: SpatialQuat;
};

export type SpatialSnapshot = {
  protocol: { name: string; version: number };
  revision: number;
  camera: SpatialCamera;
  focusedCardId: string | null;
  focusOrder: string[];
  cards: SpatialCardState[];
};

export type SpatialHit = {
  cardId: string;
  distance: number;
  point: SpatialVec3;
  local: SpatialVec3;
  u: number;
  v: number;
};

export type SpatialRay = {
  origin: SpatialVec3;
  direction: SpatialVec3;
};

export type SpatialSyncRequest = {
  cards: Array<{ id: string; pose: SpatialPose; width?: number; height?: number }>;
  focused?: string;
};

export type SpatialPoseRequest = {
  id: string;
  pose: SpatialPose;
  width?: number;
  height?: number;
};

export const SPATIAL_PROTOCOL_NAME: string;
export const SPATIAL_PROTOCOL_VERSION: number;
export const MAX_SPATIAL_CARDS: number;
export const MIN_SCALE: number;
export const WORKSPACE_CAMERA: SpatialCamera;

export function isFiniteNumber(value: unknown): value is number;
export function validatePose(pose: unknown):
  | { ok: true; value: SpatialPose }
  | { ok: false; error: { code: string; message: string } };
export function validateExtent(width: unknown, height: unknown):
  | { ok: true; value: { width: number; height: number } }
  | { ok: false; error: { code: string; message: string } };

export function quatRotateVec3(quaternion: SpatialQuat, vector: SpatialVec3): SpatialVec3;
export function quatConjugate(quaternion: SpatialQuat): SpatialQuat;
export function poseToWorld(pose: SpatialPose, local: SpatialVec3): SpatialVec3;
export function poseToLocal(pose: SpatialPose, world: SpatialVec3): SpatialVec3 | null;
export function planeNormal(pose: SpatialPose): SpatialVec3;
export function screenPointToRay(camera: SpatialCamera, x: number, y: number): SpatialRay | null;
export function rayPlaneDistance(
  ray: SpatialRay,
  planePoint: SpatialVec3,
  normal: SpatialVec3,
): number | null;
export function rayCardIntersection(ray: SpatialRay, card: SpatialCardState): SpatialHit | null;
export function hitTest(
  cards: SpatialCardState[],
  ray: SpatialRay,
  focusOrder?: string[],
): SpatialHit | null;
export function paintOrder(cards: SpatialCardState[], eye: SpatialVec3): SpatialCardState[];

export function protocolHeader(): { name: string; version: number };
export function assertProtocol(payload: unknown): SpatialSnapshot;
export function managedPoses(snapshot: SpatialSnapshot): Array<{ id: string } & SpatialPose>;

export const SPATIAL_SOURCES: { STUDIO: "studio"; HYPERVISOR: "hypervisor"; KERNEL: "kernel" };

/** One card from a `SPATIALv1` kernel wire frame (ids mapped to `card-<n>`). */
export type SpatialKernelCard = {
  id: string;
  kernelId: number;
  pose: SpatialPose;
  width: number;
  height: number;
  active: boolean;
  focused: boolean;
};

export type SpatialKernelFrame = {
  version: number;
  revision: number;
  focus: string | null;
  cards: SpatialKernelCard[];
};

export const KERNEL_FRAME_PREFIX: string;
export const KERNEL_FRAME_VERSION: number;
export function kernelCardId(id: number): string;
export function parseKernelFrame(line: string):
  | { ok: true; value: SpatialKernelFrame }
  | { ok: false; error: { code: string; message: string } };

export type SpatialClient = {
  protocol: { name: string; version: number };
  state(options?: { since?: number }): Promise<SpatialSnapshot | { unchanged: true }>;
  sync(body: SpatialSyncRequest): Promise<SpatialSnapshot>;
  setPose(body: SpatialPoseRequest): Promise<SpatialSnapshot>;
  focus(id: string): Promise<SpatialSnapshot>;
  hitTest(body: { ray: SpatialRay } | { screen: { x: number; y: number } }): Promise<{
    hit: SpatialHit | null;
    snapshot: SpatialSnapshot;
  }>;
  pointer(body: {
    x: number;
    y: number;
    pressed?: boolean;
    dragged?: boolean;
  }): Promise<{ hit: SpatialHit | null; snapshot: SpatialSnapshot }>;
};

export function createSpatialClient(options?: {
  baseUrl?: string;
  fetchImpl?: typeof fetch;
}): SpatialClient;
