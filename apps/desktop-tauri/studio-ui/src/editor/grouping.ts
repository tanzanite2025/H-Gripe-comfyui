// Pure helpers for container grouping (parent/child frames).
//
// Renderer integration lives in App/FlowCanvas, but the fiddly, bug-prone parts
// — containment geometry, absolute<->relative coordinate conversion, reparenting
// and the "parent must precede child" ordering React Flow requires — are kept
// here as pure functions so they can be unit-tested without a DOM.
//
// Groups are single-level: a group is always top-level (never nested inside
// another group), so a child's absolute position is just parent.position +
// child.position.

import type { Node } from "@xyflow/react";

export const GROUP_KIND = "group";
export const DEFAULT_GROUP_WIDTH = 320;
export const DEFAULT_GROUP_HEIGHT = 240;

export function isGroupNode(node: Node): boolean {
  return (node.data as { kind?: string } | undefined)?.kind === GROUP_KIND;
}

function sizeOf(node: Node): { width: number; height: number } {
  return {
    width: node.width ?? node.measured?.width ?? 0,
    height: node.height ?? node.measured?.height ?? 0,
  };
}

/** Absolute (canvas-space) position of a node, resolving a single parent level. */
export function absolutePosition(node: Node, byId: Map<string, Node>): { x: number; y: number } {
  if (node.parentId) {
    const parent = byId.get(node.parentId);
    if (parent) {
      return { x: parent.position.x + node.position.x, y: parent.position.y + node.position.y };
    }
  }
  return { x: node.position.x, y: node.position.y };
}

/**
 * Id of the group whose box contains the node's center, or null. Groups
 * themselves are never reparented. When several groups overlap, the last one in
 * array order (painted on top) wins.
 */
export function findContainingGroup(nodeId: string, nodes: Node[]): string | null {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const node = byId.get(nodeId);
  if (!node || isGroupNode(node)) return null;
  const abs = absolutePosition(node, byId);
  const { width, height } = sizeOf(node);
  const cx = abs.x + width / 2;
  const cy = abs.y + height / 2;
  let match: string | null = null;
  for (const g of nodes) {
    if (g.id === nodeId || !isGroupNode(g)) continue;
    const gs = sizeOf(g);
    if (cx >= g.position.x && cx <= g.position.x + gs.width && cy >= g.position.y && cy <= g.position.y + gs.height) {
      match = g.id;
    }
  }
  return match;
}

/**
 * Order nodes as: group frames, then other top-level nodes, then children.
 * Satisfies React Flow's requirement that a parent appears before its children,
 * and keeps group frames painted behind the nodes they contain.
 */
export function orderNodes(nodes: Node[]): Node[] {
  const groups = nodes.filter((n) => isGroupNode(n) && !n.parentId);
  const roots = nodes.filter((n) => !isGroupNode(n) && !n.parentId);
  const children = nodes.filter((n) => n.parentId);
  return [...groups, ...roots, ...children];
}

/**
 * Attach a node to `newParentId` (or detach when null), converting its position
 * between absolute and parent-relative space so it does not visually jump.
 * No-op when the parent is unchanged. Result is re-ordered for React Flow.
 */
export function reparentNode(nodes: Node[], nodeId: string, newParentId: string | null): Node[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const node = byId.get(nodeId);
  if (!node) return nodes;
  if ((node.parentId ?? null) === newParentId) return nodes;

  const abs = absolutePosition(node, byId);
  let position = abs;
  if (newParentId) {
    const parent = byId.get(newParentId);
    if (parent) position = { x: abs.x - parent.position.x, y: abs.y - parent.position.y };
  }
  const updated: Node = { ...node, position };
  if (newParentId) updated.parentId = newParentId;
  else delete updated.parentId;

  return orderNodes(nodes.map((n) => (n.id === nodeId ? updated : n)));
}

/**
 * Detach every child of the given groups, converting each back to absolute
 * coordinates. Used when a group frame is deleted so its members survive as
 * free top-level nodes instead of becoming orphans.
 */
export function detachChildren(nodes: Node[], groupIds: Set<string>): Node[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  return nodes.map((n) => {
    if (n.parentId && groupIds.has(n.parentId)) {
      const copy: Node = { ...n, position: absolutePosition(n, byId) };
      delete copy.parentId;
      return copy;
    }
    return n;
  });
}

/** A fresh, resizable group frame node. */
export function makeGroupNode(id: string, x: number, y: number, label = "Group"): Node {
  return {
    id,
    type: "group",
    position: { x, y },
    width: DEFAULT_GROUP_WIDTH,
    height: DEFAULT_GROUP_HEIGHT,
    data: { kind: GROUP_KIND, params: { label }, status: "idle" },
  } as Node;
}
