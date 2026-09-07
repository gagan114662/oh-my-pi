#!/usr/bin/env bun
/** Production-process edit benchmark adapter. See metaharness/README.md. */
import { main } from "./metaharness/adapter";
if (import.meta.main) await main(Bun.argv.slice(2));
