#!/usr/bin/env node
/**
 * Per-clone Git configuration.
 *
 * .gitattributes travels with the repo and fully controls line endings, but
 * these settings live in .git/config, which is NOT cloned. They must be
 * re-applied in every working copy. Cargo has no install hook to run this
 * automatically, so run it once after cloning:
 *
 *   node scripts/setup-git.ts
 */
import { execFileSync } from 'node:child_process';

const SETTINGS: [key: string, value: string, why: string][] = [
  [
    'core.autocrlf',
    'false',
    'Never rewrite line endings behind our back -- .gitattributes owns this',
  ],
  ['core.eol', 'lf', 'Fallback for files .gitattributes does not match'],
  [
    'core.ignorecase',
    'false',
    'Treat Foo.rs and foo.rs as distinct (source dir is case-sensitive)',
  ],
  ['core.longpaths', 'true', 'Paths under target/ can exceed the 260-char Windows MAX_PATH'],
  ['core.untrackedCache', 'true', 'Faster `git status`'],
  ['diff.algorithm', 'histogram', 'Noticeably better diffs than the default myers'],
  ['merge.conflictstyle', 'zdiff3', 'Conflict markers include the common ancestor'],
  ['fetch.prune', 'true', 'Delete local refs for branches deleted on the remote'],
  ['push.autoSetupRemote', 'true', 'First `git push` on a new branch just works'],
];

const git = (args: string[]): string => execFileSync('git', args, { encoding: 'utf8' }).trim();

let changed = 0;
let failed = 0;

for (const [key, value, why] of SETTINGS) {
  let current: string | null = null;
  try {
    current = git(['config', '--local', '--get', key]);
  } catch {
    // Exit code 1 simply means "not set locally".
  }

  if (current === value) continue;

  try {
    git(['config', '--local', key, value]);
    console.log(`  set ${key} = ${value}\n      ${why}`);
    changed += 1;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`  skipped ${key}: ${message.split('\n')[0]}`);
    failed += 1;
  }
}

if (changed === 0 && failed === 0) {
  console.log('git: already configured');
} else {
  console.log(`git: ${changed} setting(s) applied${failed ? `, ${failed} skipped` : ''}`);
}

// Warn loudly if the working tree disagrees with .gitattributes -- this is the
// classic "whole repo shows as modified" symptom after a bad clone.
try {
  const renormalized = git(['diff', '--stat']);
  if (renormalized) {
    console.warn(
      '\n  Warning: working tree has uncommitted changes.\n' +
        '  If these are pure line-ending diffs, run: git add --renormalize .',
    );
  }
} catch {
  // Not fatal -- this is advisory only.
}
