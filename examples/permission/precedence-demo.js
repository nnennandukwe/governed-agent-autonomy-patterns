'use strict';

const policy = {
  allow: ['git status', 'npm test'],
  ask: ['git push', 'rm *'],
  deny: ['rm -rf /'],
  dangerousPaths: ['/', '/System', '/usr'],
};

function startsWithAny(value, patterns) {
  return patterns.some(pattern => value.startsWith(pattern.replace(' *', '')));
}

function decide(command, targetPath) {
  if (policy.deny.includes(command)) {
    return 'deny';
  }

  if (policy.dangerousPaths.includes(targetPath)) {
    return 'ask';
  }

  if (startsWithAny(command, policy.ask)) {
    return 'ask';
  }

  if (policy.allow.includes(command)) {
    return 'allow';
  }

  return 'ask';
}

const samples = [
  ['git status', '/Users/demo/project'],
  ['git push', '/Users/demo/project'],
  ['rm -rf /', '/'],
  ['rm cache', '/usr'],
];

for (const [command, targetPath] of samples) {
  console.log(`${command} @ ${targetPath} -> ${decide(command, targetPath)}`);
}
