import { describe, it, expect } from 'vitest';
import { moveTargets, UNCATEGORISED } from '../../../../../crates/vxn-core-ui-web/assets/preset-browser.js';

// `moveTargets(currentName, corpus)` builds the user-side context-menu
// "Move to ▸" list. Root (`name: null`) leads when present and the user is
// not already at root; named folders follow in alpha-insensitive order; the
// current folder is always excluded.
describe('moveTargets', () => {
  it('returns an empty list for an empty corpus', () => {
    expect(moveTargets(null, { user: [] })).toEqual([]);
    expect(moveTargets(null, undefined)).toEqual([]);
  });

  it('excludes the current folder from the list', () => {
    const corpus = { user: [{ name: 'Bass' }, { name: 'Pad' }, { name: 'Lead' }] };
    const names = moveTargets('Bass', corpus).map((t) => t.name);
    expect(names).not.toContain('Bass');
    expect(names).toEqual(['Lead', 'Pad']);
  });

  it('includes the virtual root when present and currentName is non-null', () => {
    const corpus = { user: [{ name: null }, { name: 'Bass' }] };
    const list = moveTargets('Bass', corpus);
    expect(list[0]).toEqual({ name: null, label: UNCATEGORISED });
  });

  it('omits the virtual root when currentName is null (no-op move)', () => {
    const corpus = { user: [{ name: null }, { name: 'Bass' }] };
    const list = moveTargets(null, corpus);
    expect(list.every((t) => t.name !== null)).toBe(true);
    expect(list.map((t) => t.name)).toEqual(['Bass']);
  });

  it('sorts named folders case-insensitively', () => {
    const corpus = { user: [{ name: 'pad' }, { name: 'Bass' }, { name: 'lead' }] };
    expect(moveTargets(null, corpus).map((t) => t.name)).toEqual(['Bass', 'lead', 'pad']);
  });
});
