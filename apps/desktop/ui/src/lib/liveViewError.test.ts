import assert from "node:assert/strict";
import { test } from "node:test";
import { liveViewErrorCode } from "./liveViewError.ts";

test("liveViewErrorCode maps transport and HTTP statuses to stable error codes", () => {
  const cases: Array<[{ code: string; status: number }, string]> = [
    [{ code: "network", status: 500 }, "network"],
    [{ code: "anything", status: 0 }, "network"],
    [{ code: "timeout", status: 0 }, "timeout"],
    [{ code: "forbidden", status: 401 }, "unauthorized"],
    [{ code: "internal", status: 502 }, "server_restarting"],
    [{ code: "internal", status: 504 }, "timeout"],
    [{ code: "pause_changed", status: 409 }, "pause_changed"],
    [{ code: "", status: 418 }, ""]
  ];
  for (const [input, expected] of cases) {
    assert.equal(liveViewErrorCode(input), expected, JSON.stringify(input));
  }
});
