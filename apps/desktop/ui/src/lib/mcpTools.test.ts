import assert from "node:assert/strict";
import { test } from "node:test";
import { nextExclude, toolRows } from "./mcpTools.ts";

test("lists excluded literal tools as off and refuses to touch an allow-list", () => {
  const tools = [{ name: "mcp:gh/create_issue", description: "d", effect: "write" }];
  assert.deepEqual(toolRows(tools, ["delete_repo", "list_*"], []), [
    { tool: "create_issue", enabled: true, effect: "write", description: "d" },
    { tool: "delete_repo", enabled: false, effect: "", description: "" }
  ]);
  assert.equal(toolRows(tools, [], ["create_*"]), null);
});

test("toggles one tool in tool_exclude without losing patterns", () => {
  assert.deepEqual(nextExclude(["list_*"], "a", false), ["list_*", "a"]);
  assert.deepEqual(nextExclude(["list_*", "a"], "a", true), ["list_*"]);
});
