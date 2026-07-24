import assert from 'node:assert/strict';
import test from 'node:test';

import {
  LifecycleMachine,
  LifecycleTransitionError,
} from '../src/lifecycle.js';

test('a trial advances only through the declared lifecycle', () => {
  const lifecycle = new LifecycleMachine();

  assert.equal(lifecycle.transition('prepare'), 'preparing');
  assert.equal(lifecycle.transition('begin_planning'), 'planning');
  assert.equal(lifecycle.transition('request_approval'), 'awaiting_approval');
  assert.equal(lifecycle.transition('approve'), 'executing');
  assert.equal(lifecycle.transition('begin_verification'), 'verifying');
  assert.equal(lifecycle.transition('finish'), 'terminal');
});

test('an illegal lifecycle transition fails closed with recovery guidance', () => {
  const lifecycle = new LifecycleMachine();

  assert.throws(
    () => lifecycle.transition('finish'),
    (error: unknown) => {
      assert.ok(error instanceof LifecycleTransitionError);
      assert.equal(error.code, 'lifecycle.invalid_transition');
      assert.match(error.message, /Cannot apply finish while trial is created/);
      assert.match(error.recovery, /resume from created/);
      return true;
    },
  );
  assert.equal(lifecycle.phase, 'created');
});
