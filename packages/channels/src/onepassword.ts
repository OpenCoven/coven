import { execFile as execFileCallback } from "node:child_process";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);

export type OnePasswordRead = (reference: string) => Promise<string | null | undefined>;
export type OnePasswordCliRead = (
  args: string[],
  env: Record<string, string | undefined> | undefined,
) => Promise<string>;

export interface OnePasswordReadOptions {
  env?: Record<string, string | undefined>;
  readOnePassword?: OnePasswordRead;
  runOnePasswordCli?: OnePasswordCliRead;
}

export function resolveOnePasswordReference(
  envName: string,
  options: OnePasswordReadOptions = {},
): string | null {
  return (options.env ?? process.env)[envName]?.trim() || null;
}

export async function readOnePasswordReference(
  reference: string,
  options: OnePasswordReadOptions = {},
): Promise<string> {
  let resolved: string | null | undefined;

  try {
    resolved = options.readOnePassword
      ? await options.readOnePassword(reference)
      : await readOnePasswordCli(reference, options);
  } catch {
    throw new Error("1Password reference could not be read.");
  }

  const trimmed = resolved?.trim();

  if (!trimmed) {
    throw new Error("1Password reference resolved to an empty value.");
  }

  return trimmed;
}

async function readOnePasswordCli(
  reference: string,
  options: OnePasswordReadOptions,
): Promise<string> {
  const op = options.runOnePasswordCli ?? runOnePasswordCli;
  const itemReference = parseItemTitleReference(reference);

  if (itemReference) {
    return op(
      [
        "item",
        "get",
        itemReference.item,
        "--vault",
        itemReference.vault,
        "--fields",
        `label=${itemReference.field}`,
        "--reveal",
      ],
      options.env,
    );
  }

  return op(["read", reference], options.env);
}

async function runOnePasswordCli(
  args: string[],
  env: Record<string, string | undefined> | undefined,
): Promise<string> {
  const { stdout } = await execFile("op", args, {
    encoding: "utf8",
    env: env ? { ...process.env, ...env } : process.env,
    maxBuffer: 1024 * 1024,
  });

  return stdout;
}

function parseItemTitleReference(
  reference: string,
): { vault: string; item: string; field: string } | null {
  const match = /^op:\/\/([^/]+)\/([^/]+)\/([^/]+)$/.exec(reference);
  if (!match) {
    return null;
  }

  const [, vault, item, field] = match;

  if (!item?.includes("|") || !vault || !field) {
    return null;
  }

  return { vault, item, field };
}
