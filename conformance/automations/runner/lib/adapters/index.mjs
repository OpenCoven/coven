// Target adapters for the conformance plane.
//
// Finding 3 of the #882 review: CI exercised only the JavaScript reference
// oracle while the report language implied product certification. The
// reference-oracle runs are vector self-tests; certifying a real product
// requires one of these adapters against a target that advertises
// `coven.automations.conformance.v1`:
//
//   daemon            a running daemon endpoint over HTTP
//                     (COVEN_CONFORMANCE_ENDPOINT, e.g. http://127.0.0.1:7600)
//   in-process        a linked Coven implementation module exporting
//                     probe()/evaluate() (COVEN_CONFORMANCE_INPROCESS_MODULE)
//   packaged-release  a packed npm artifact's binary
//                     (COVEN_CONFORMANCE_PACKAGE_BIN)
//
// Until an adapter is wired and its probe succeeds, every vector on that
// target is not-applicable and the certification gate fails — scaffolding
// can never read as a passing certification.

export const TARGET_KINDS = ['reference-oracle', 'in-process', 'daemon', 'packaged-release'];
export const TARGETS_CAPABILITY = 'coven.automations.conformance.v1';

export function createTarget(kind, { definitionSchema, endpoint = null } = {}) {
  if (kind === 'reference-oracle') {
    return {
      kind,
      definitionSchema,
      capabilities: ['reference-oracle']
    };
  }
  if (kind === 'daemon') return createDaemonTarget({ endpoint });
  if (kind === 'in-process') return createInProcessTarget();
  if (kind === 'packaged-release') return createPackagedReleaseTarget();
  throw new Error(`unknown target kind: ${kind}`);
}

function capabilityAdvertisement(capability) {
  if (!capability || typeof capability !== 'object') return false;
  return (
    capability.capability === TARGETS_CAPABILITY &&
    Array.isArray(capability.profiles) &&
    capability.profiles.length > 0
  );
}

// Daemon adapter: HTTP endpoint convention (see conformance/automations/README.md).
//   GET  {endpoint}/conformance/v1/capability -> {capability, profiles, ...}
//   POST {endpoint}/conformance/v1/evaluate   <- vector, -> {status, failures}
export function createDaemonTarget({ endpoint } = {}) {
  return {
    kind: 'daemon',
    capabilities: [],
    async probe() {
      if (!endpoint) return null;
      let response;
      try {
        response = await fetch(`${endpoint.replace(/\/$/, '')}/conformance/v1/capability`, {
          signal: AbortSignal.timeout(5000)
        });
      } catch {
        return null; // endpoint unreachable: nothing executed on this target
      }
      if (!response.ok) return null;
      try {
        const capability = await response.json();
        return capabilityAdvertisement(capability) ? capability : null;
      } catch {
        return null;
      }
    },
    async evaluate(vector) {
      const response = await fetch(`${endpoint.replace(/\/$/, '')}/conformance/v1/evaluate`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(vector),
        signal: AbortSignal.timeout(60_000)
      });
      if (!response.ok) {
        return {
          status: 'failed',
          failures: [
            {
              vectorId: vector.vectorId,
              profile: vector.profile,
              invariant: 'target-evaluator',
              objectIds: [],
              eventCursor: null,
              expected: 'the daemon endpoint evaluates the vector',
              observed: `POST /conformance/v1/evaluate returned HTTP ${response.status}`,
              reproduction: `node conformance/automations/runner/conformance.mjs --target daemon --vector ${vector.vectorId}`
            }
          ]
        };
      }
      return response.json();
    }
  };
}

// In-process adapter: a linked implementation module that exports
// probe() -> capability advertisement (or null) and evaluate(vector) ->
// {status, failures}. Loading Coven internals is the implementation's
// choice; the plane stays dependency-free.
export function createInProcessTarget() {
  const modulePath = process.env.COVEN_CONFORMANCE_INPROCESS_MODULE ?? null;
  let loaded = null;
  return {
    kind: 'in-process',
    capabilities: [],
    async probe() {
      if (!modulePath) return null;
      if (loaded === null) {
        try {
          loaded = await import(modulePath);
        } catch {
          return null;
        }
      }
      if (typeof loaded.probe !== 'function') return null;
      try {
        const capability = await loaded.probe();
        return capabilityAdvertisement(capability) ? capability : null;
      } catch {
        return null;
      }
    },
    async evaluate(vector) {
      if (loaded === null || typeof loaded.evaluate !== 'function') return null;
      return loaded.evaluate(vector);
    }
  };
}

// Packaged-release adapter: the packed artifact's binary must implement
//   <bin> automations conformance capability  -> capability advertisement JSON
//   <bin> automations conformance evaluate    <- vector on stdin -> report JSON
export function createPackagedReleaseTarget() {
  const bin = process.env.COVEN_CONFORMANCE_PACKAGE_BIN ?? null;
  return {
    kind: 'packaged-release',
    capabilities: [],
    async probe() {
      if (!bin) return null;
      const { runPackaged } = await import('./packaged-release.mjs');
      const capability = runPackaged(bin, ['automations', 'conformance', 'capability']);
      return capabilityAdvertisement(capability) ? capability : null;
    },
    async evaluate(vector) {
      const { runPackaged } = await import('./packaged-release.mjs');
      return runPackaged(bin, ['automations', 'conformance', 'evaluate'], {
        input: JSON.stringify(vector)
      });
    }
  };
}
