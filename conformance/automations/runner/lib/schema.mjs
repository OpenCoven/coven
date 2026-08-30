// Minimal JSON Schema (draft 2020-12 subset) validator for the conformance
// plane. The runner must stay dependency-free and consumable without linking
// Coven private modules, so it validates vectors, artifacts, and reports with
// this self-contained checker instead of pulling a validator package.
//
// Supported keywords: type, required, properties, additionalProperties, enum,
// const, items, minItems, maxItems, minLength, maxLength, pattern, minimum,
// maximum, oneOf, anyOf, allOf, and local `$ref: "#/$defs/..."` pointers.
// Meta keywords (title, description, $schema, $id) are ignored.

const SUPPORTED_TYPES = ['string', 'number', 'integer', 'boolean', 'object', 'array', 'null'];

function typeOf(value) {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (Number.isInteger(value)) return 'integer';
  return typeof value;
}

function typeMatches(value, expected) {
  const actual = typeOf(value);
  if (expected === 'number') return actual === 'number' || actual === 'integer';
  if (expected === 'integer') return actual === 'integer';
  return actual === expected;
}

function resolveRef(root, ref) {
  if (typeof ref !== 'string' || !ref.startsWith('#/')) {
    throw new Error(`unsupported $ref (only local "#/..." pointers are supported): ${ref}`);
  }
  let node = root;
  for (const segment of ref.slice(2).split('/')) {
    if (node === undefined || node === null || typeof node !== 'object') {
      throw new Error(`$ref path does not resolve: ${ref}`);
    }
    node = node[segment];
  }
  if (node === undefined) throw new Error(`$ref path does not resolve: ${ref}`);
  return node;
}

// Validates `value` against `schemaNode` (with `root` for $ref resolution).
// Returns an array of human-readable error strings with JSON-path prefixes;
// an empty array means valid.
export function validateAgainstSchema(value, schemaNode, root = schemaNode, path = '$') {
  if (schemaNode === true) return [];
  if (schemaNode === false) return [`${path}: schema forbids any value`];
  if (typeof schemaNode !== 'object' || schemaNode === null || Array.isArray(schemaNode)) {
    return [`${path}: schema node is not an object`];
  }

  if (typeof schemaNode.$ref === 'string') {
    return validateAgainstSchema(value, resolveRef(root, schemaNode.$ref), root, path);
  }

  const errors = [];

  if (schemaNode.type !== undefined) {
    const expected = Array.isArray(schemaNode.type) ? schemaNode.type : [schemaNode.type];
    for (const type of expected) {
      if (!SUPPORTED_TYPES.includes(type)) {
        throw new Error(`unsupported schema type: ${type}`);
      }
    }
    if (!expected.some((type) => typeMatches(value, type))) {
      errors.push(`${path}: expected type ${expected.join('|')}, got ${typeOf(value)}`);
    }
  }

  if (schemaNode.const !== undefined && JSON.stringify(value) !== JSON.stringify(schemaNode.const)) {
    errors.push(`${path}: expected const ${JSON.stringify(schemaNode.const)}, got ${JSON.stringify(value)}`);
  }

  if (schemaNode.enum !== undefined) {
    const matches = schemaNode.enum.some(
      (option) => JSON.stringify(option) === JSON.stringify(value)
    );
    if (!matches) {
      errors.push(`${path}: value ${JSON.stringify(value)} is not one of ${JSON.stringify(schemaNode.enum)}`);
    }
  }

  if (typeof value === 'string') {
    if (schemaNode.minLength !== undefined && value.length < schemaNode.minLength) {
      errors.push(`${path}: string shorter than minLength ${schemaNode.minLength}`);
    }
    if (schemaNode.maxLength !== undefined && value.length > schemaNode.maxLength) {
      errors.push(`${path}: string longer than maxLength ${schemaNode.maxLength}`);
    }
    if (schemaNode.pattern !== undefined && !new RegExp(schemaNode.pattern).test(value)) {
      errors.push(`${path}: string does not match pattern ${schemaNode.pattern}`);
    }
  }

  if (typeof value === 'number' && typeMatches(value, 'number')) {
    if (schemaNode.minimum !== undefined && value < schemaNode.minimum) {
      errors.push(`${path}: number below minimum ${schemaNode.minimum}`);
    }
    if (schemaNode.maximum !== undefined && value > schemaNode.maximum) {
      errors.push(`${path}: number above maximum ${schemaNode.maximum}`);
    }
  }

  if (Array.isArray(value)) {
    if (schemaNode.minItems !== undefined && value.length < schemaNode.minItems) {
      errors.push(`${path}: array has fewer than ${schemaNode.minItems} items`);
    }
    if (schemaNode.maxItems !== undefined && value.length > schemaNode.maxItems) {
      errors.push(`${path}: array has more than ${schemaNode.maxItems} items`);
    }
    if (schemaNode.items !== undefined) {
      value.forEach((item, index) => {
        errors.push(
          ...validateAgainstSchema(item, schemaNode.items, root, `${path}[${index}]`)
        );
      });
    }
  }

  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const required = Array.isArray(schemaNode.required) ? schemaNode.required : [];
    for (const key of required) {
      if (!(key in value)) errors.push(`${path}: missing required property "${key}"`);
    }
    if (schemaNode.properties !== undefined) {
      for (const [key, subschema] of Object.entries(schemaNode.properties)) {
        if (key in value) {
          errors.push(...validateAgainstSchema(value[key], subschema, root, `${path}.${key}`));
        }
      }
    }
    if (schemaNode.additionalProperties !== undefined) {
      const known = new Set(Object.keys(schemaNode.properties ?? {}));
      for (const key of Object.keys(value)) {
        if (known.has(key)) continue;
        if (schemaNode.additionalProperties === false) {
          errors.push(`${path}: additional property "${key}" is not allowed`);
        } else if (typeof schemaNode.additionalProperties === 'object') {
          errors.push(
            ...validateAgainstSchema(
              value[key],
              schemaNode.additionalProperties,
              root,
              `${path}.${key}`
            )
          );
        }
      }
    }
  }

  for (const keyword of ['oneOf', 'anyOf']) {
    if (schemaNode[keyword] !== undefined) {
      const branches = schemaNode[keyword];
      const branchErrors = branches.map((branch) =>
        validateAgainstSchema(value, branch, root, path)
      );
      const passing = branchErrors.filter((branch) => branch.length === 0).length;
      if (keyword === 'oneOf' && passing !== 1) {
        errors.push(`${path}: expected exactly one oneOf branch to match, got ${passing}`);
      }
      if (keyword === 'anyOf' && passing === 0) {
        errors.push(`${path}: no anyOf branch matched`);
      }
    }
  }

  if (Array.isArray(schemaNode.allOf)) {
    for (const branch of schemaNode.allOf) {
      errors.push(...validateAgainstSchema(value, branch, root, path));
    }
  }

  return errors;
}

// Convenience wrapper: throws when invalid, with all errors joined.
export function assertValid(value, schema, label) {
  const errors = validateAgainstSchema(value, schema);
  if (errors.length > 0) {
    throw new Error(`${label} failed schema validation:\n  ${errors.join('\n  ')}`);
  }
}
