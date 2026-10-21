import { expect, test } from "vitest";
import { artifact, uniqueHash } from "../support/artifacts.ts";
import { requestLines } from "../support/logs.ts";
import { startServer } from "../support/nx-cache-aws.ts";
import { withVariables } from "../support/server-env.ts";
import { e2eServerEnv } from "./storage/shared-storage.ts";

test("each cache outcome is logged with its byte count", async () => {
    const server = await startServer(withVariables(e2eServerEnv(), { LOG_FORMAT: "json" }));
    const client = server.client("read-write");
    const hash = uniqueHash("outcomes");
    const body = artifact(1024);

    await client.put(hash, body);
    await client.put(hash, body);
    await client.get(hash);
    await client.get(uniqueHash("outcomes-missing"));
    const lines = requestLines((await server.stop()).stdout);

    expect(lines).toMatchObject([
        { outcome: "stored", status: 200, bytes: 1024, access: "read_write" },
        { outcome: "exists", status: 409, bytes: 0 },
        { outcome: "hit", status: 200, bytes: 1024 },
        { outcome: "miss", status: 404, bytes: 0 },
    ]);
});
