import { describe, it, expect } from 'vitest';
import { folderValue } from '../../../../../crates/vxn-core-ui-web/assets/preset-browser.js';

// `folderValue` is the inverse of the Save As `<select>` value → folder
// name mapping. The virtual user-root has no name; we sentinel it as
// `__root__` so the option carries the human label without colliding with
// a real folder.
describe('folderValue', () => {
  it('maps a null folder name to the __root__ sentinel', () => {
    expect(folderValue(null)).toBe('__root__');
  });

  it('passes a named folder through unchanged', () => {
    expect(folderValue('Bass')).toBe('Bass');
    expect(folderValue('Lead 1')).toBe('Lead 1');
  });

  it('treats undefined like null (defensive)', () => {
    expect(folderValue(undefined)).toBe('__root__');
  });
});
