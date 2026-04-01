'use strict';

function validateReport(report) {
  const hasCheck = report.includes('### Check:');
  const hasCommand = report.includes('**Command run:**');
  const hasOutput = report.includes('**Output observed:**');
  const verdictMatch = report.match(/VERDICT: (PASS|FAIL|PARTIAL)$/m);

  return {
    hasCheck,
    hasCommand,
    hasOutput,
    verdict: verdictMatch ? verdictMatch[1] : null,
    valid: hasCheck && hasCommand && hasOutput && Boolean(verdictMatch),
  };
}

const sampleReport = `
### Check: CLI prints usage
**Command run:**
  node cli.js --help
**Output observed:**
  usage: cli [options]
**Result: PASS**

VERDICT: PASS
`.trim();

console.log(validateReport(sampleReport));
