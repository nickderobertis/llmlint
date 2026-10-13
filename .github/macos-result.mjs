// GitHub job conclusions are the third-party boundary; never infer success from a skip.
const needs = JSON.parse(process.env.NEEDS_JSON ?? '{}');
const policy = needs.policy;
if (policy?.result !== 'success' || needs.gate?.result !== 'success') throw new Error('policy/gate failed, cancelled or missing');
const required = policy.outputs?.macosRequired;
if (!['true', 'false'].includes(required)) throw new Error('missing classifier decision');
if (required === 'true') {
  if (needs.macos?.result !== 'success') throw new Error('required Mac proof failed, cancelled or missing');
  console.log('Mac cross and install assertions succeeded');
} else {
  if (needs.macos?.result !== 'skipped') throw new Error('unexpected Mac result for authorized omission');
  console.log(`Mac lane intentionally omitted: ${policy.outputs.reason}`);
}
