import { expect, test } from "vitest";
import { artifact, uniqueHash } from "../support/artifacts.ts";
import { MAX_HASH_LENGTH } from "../support/client.ts";
import { type LogLine, requestLines } from "../support/logs.ts";
import { startServer } from "../support/nx-cache-aws.ts";
import {
    READ_ONLY_TOKEN,
    READ_WRITE_TOKEN,
    type ServerEnv,
    VALID_ENV,
    withVariables,
} from "../support/server-env.ts";

const JSON_ENV = withVariables(VALID_ENV, { LOG_FORMAT: "json" });

async function onlyRequestLine(
    env: ServerEnv,
    send: (server: Awaited<ReturnType<typeof startServer>>) => Promise<unknown>,
): Promise<LogLine> {
    const server = await startServer(env);

    await send(server);
    const lines = requestLines((await server.stop()).stdout);

    const [line, ...others] = lines;
    if (line === undefined || others.length > 0) {
        throw new Error(`expected exactly one request line, got ${lines.length}`);
    }
    return line;
}

test("a request without a token logs unauthorized", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("none").get("some-hash"),
    );

    expect(line).toMatchObject({
        method: "GET",
        hash: "some-hash",
        status: 401,
        outcome: "unauthorized",
        access: "none",
        level: "INFO",
    });
});

test("a read-only upload logs forbidden", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("read-only").put(uniqueHash("forbidden"), artifact(1024)),
    );

    expect(line).toMatchObject({
        method: "PUT",
        status: 403,
        outcome: "forbidden",
        access: "read_only",
    });
});

test("an invalid hash is logged without the hash", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("read-write").get("a.b"),
    );

    expect(line).toMatchObject({ status: 400, outcome: "invalid", access: "read_write" });
    expect(line.hash).toBeUndefined();
});

test("a too-long hash is not logged", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("read-write").get("a".repeat(MAX_HASH_LENGTH + 1)),
    );

    expect(line.status).toBe(400);
    expect(line.hash).toBeUndefined();
});

test("a percent-encoded hash is logged decoded", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("none").request("GET", "/v1/cache/abc-%C3%A9t%C3%A9"),
    );

    expect(line.hash).toBe("abc-été");
});

test("an unknown route logs invalid", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("none").request("GET", "/nope"),
    );

    expect(line).toMatchObject({ status: 404, outcome: "invalid" });
});

test("health checks are logged only at debug", async () => {
    const atInfo = await startServer(JSON_ENV);
    await atInfo.client("none").request("GET", "/health");
    const infoLines = requestLines((await atInfo.stop()).stdout);

    const atDebug = await startServer(withVariables(JSON_ENV, { DEBUG: "true" }));
    await atDebug.client("none").request("GET", "/health");
    const debugLines = requestLines((await atDebug.stop()).stdout);

    expect(infoLines.filter((line) => line.outcome === "health")).toEqual([]);
    expect(debugLines.filter((line) => line.outcome === "health")).toEqual([
        expect.objectContaining({ level: "DEBUG", status: 200 }),
    ]);
});

test("a storage failure logs an ERROR request line", async () => {
    const line = await onlyRequestLine(JSON_ENV, (server) =>
        server.client("read-write").get(uniqueHash("storage-failure")),
    );

    expect(line).toMatchObject({ status: 500, outcome: "error", level: "ERROR" });
});

test("tokens never reach the logs", async () => {
    const server = await startServer(JSON_ENV);
    const hash = uniqueHash("tokens");

    await server.client("read-write").get(hash);
    await server.client("read-only").put(hash, artifact(16));
    await server.client({ authorization: "Bearer some-guess-123" }).get(hash);
    await server.client({ authorization: "Basic c2VjcmV0" }).get(hash);
    const logs = await server.stop();

    const output = logs.stdout + logs.stderr;
    for (const secret of [READ_WRITE_TOKEN, READ_ONLY_TOKEN, "some-guess-123", "c2VjcmV0"]) {
        expect(output).not.toContain(secret);
    }
});
