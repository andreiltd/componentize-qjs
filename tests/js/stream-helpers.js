export async function verifyHelpers() {
    await withStream(verifyPartialWrites);
    await withStream(verifyEmptyAndClosedWrites);
    await withStream(verifyClosureAfterProgress);
    await withStream(verifyClosureWithoutProgress);
    await withStream(verifyIncompleteIterable);
    await withStream(verifyThenables);
    await withStream(verifyWriteErrors);
}

async function withStream(verify) {
    const { readable, writable } = wit.Stream(wit.Stream.U8);

    try {
        await verify(writable);
    } finally {
        writable.drop();
        readable.drop();
    }
}

async function verifyPartialWrites(writable) {
    const chunks = [];
    writable.write = async buffer => {
        chunks.push(Array.from(buffer));
        return Math.min(2, buffer.length);
    };

    assert(await writable.writeAll(new Uint8Array([1, 2, 3, 4, 5])) === 5, "partial total");
    assert(JSON.stringify(chunks) === "[[1,2,3,4,5],[3,4,5],[5]]", "partial suffixes");

    assert(await writable.writeAll([6, 7, 8]) === 3, "plain-array batch");
    assert(await writable.writeIterableItem(new Uint8Array([6, 7, 8])), "typed-array batch");
}

async function verifyEmptyAndClosedWrites(writable) {
    writable.write = () => { throw new Error("empty or closed stream must not write"); };

    assert(writable.writeAll([]) === 0, "empty writes retain their immediate return shape");

    const empty = writable.writeIterableItem(new Uint8Array());
    assert(empty instanceof Promise && await empty, "empty batch resolves true");

    writable.drop();
    assert(writable.writeAll(42) === 0, "closed writes do not inspect the buffer");

    const closed = writable.writeIterableItem([1, 2]);
    assert(closed instanceof Promise && await closed === false, "closed iterable result");
}

async function verifyClosureAfterProgress(writable) {
    writable.write = async function () {
        this.drop();
        return 1;
    };

    assert(await writable.writeAll([1, 2]) === 1, "partial closure total");
}

async function verifyClosureWithoutProgress(writable) {
    writable.write = async function () {
        this.drop();
        return 0;
    };

    assert(await writable.writeAll([1]) === 0, "closure without progress");
}

async function verifyIncompleteIterable(writable) {
    writable.write = async function () {
        this.drop();
        return 1;
    };

    assert(await writable.writeIterableItem(new Uint8Array([1, 2])) === false, "partial iterable result");
}

async function verifyThenables(writable) {
    writable.write = buffer => ({ then: next => next(buffer.length) });

    const mapped = writable.writeIterableItem(new Uint8Array([1, 2]));
    assert(mapped instanceof Promise && await mapped, "immediate thenable completion resolves true");

    writable.write = () => ({
        then(next) {
            next(1);
            return next(1);
        }
    });

    await rejects(() => writable.writeAll([1]), "write buffer");
}

async function verifyWriteErrors(writable) {
    const write = () => writable.writeAll([1]);

    writable.write = async () => 0;
    await rejects(() => writable.writeAll(42), "array");
    await rejects(write, "made no progress");

    writable.write = async () => 2;
    await rejects(write, "2 items for a 1-item buffer");

    writable.write = () => 1;
    await rejects(write, "promise");

    writable.write = async () => { throw new Error("write failed"); };
    await rejects(write, "write failed");
}

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

async function rejects(call, message) {
    try {
        await call();
    } catch (error) {
        assert(error.message.includes(message), error.message);
        return;
    }

    throw new Error(`expected rejection: ${message}`);
}

export async function unusedStream() {
    throw new Error("type declaration only");
}
