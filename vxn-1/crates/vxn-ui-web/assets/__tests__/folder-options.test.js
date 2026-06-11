import { describe, it, expect } from 'vitest';
import { folderOptions, UNCATEGORISED } from '../../../../../crates/vxn-core-ui-web/assets/preset-browser.js';

// `folderOptions(corpus)` populates the Save As folder dropdown: always
// leads with the virtual root sentinel, followed by alpha-insensitive
// named folders. Mirrors the left-pane order in the browser panel.
describe('folderOptions', () => {
  it('always offers the root option, even on an empty corpus', () => {
    const opts = folderOptions({ user: [] });
    expect(opts[0]).toEqual({ value: '__root__', label: UNCATEGORISED });
  });

  it('defends against a missing corpus argument', () => {
    expect(folderOptions(undefined)).toEqual([
      { value: '__root__', label: UNCATEGORISED },
    ]);
  });

  it('lists named folders alpha-insensitive after the root', () => {
    const corpus = { user: [{ name: 'pad' }, { name: 'Bass' }, { name: 'lead' }] };
    const opts = folderOptions(corpus);
    expect(opts.map((o) => o.value)).toEqual(['__root__', 'Bass', 'lead', 'pad']);
  });

  it('does not duplicate the virtual root even when the corpus has one', () => {
    // A root entry in `corpus.user` (name: null) is the same virtual
    // folder the sentinel represents — the option list must not double-up.
    const corpus = { user: [{ name: null }, { name: 'Bass' }] };
    const values = folderOptions(corpus).map((o) => o.value);
    expect(values.filter((v) => v === '__root__').length).toBe(1);
    expect(values).toEqual(['__root__', 'Bass']);
  });
});
