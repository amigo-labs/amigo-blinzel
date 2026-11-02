// Not a hook: the spec skills run it as a context command and read the commit
// mode from its one line of output. Any failure prints "on", the default; the
// skill context falls back to it as well.

import { commitMode } from './lib.mjs';

const ON = 'on - commit without asking, as the skill describes.';
const DISABLE_HINT = 'To be asked before every commit, set "env": { "SPEC_AUTOCOMMIT": "false" } in .claude/settings.local.json.';

try {
  const mode = commitMode();
  if (mode.autoCommit) {
    const problem = mode.problem ? ` Ignored: ${mode.problem}.` : '';
    console.log(`${ON} ${DISABLE_HINT}${problem}`);
  } else {
    console.log('ask - before every commit ask via AskUserQuestion: "Commit now" or continue without commit (SPEC_AUTOCOMMIT=false).');
  }
} catch {
  console.log(`${ON} ${DISABLE_HINT}`);
}
