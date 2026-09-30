wit.Stream.from = function(iterable, type) {
    const { readable, writable } = wit.Stream(type);
    const completion = (async () => {
        try {
            for await (const item of iterable) {
                if (!await writable.writeIterableItem(item)) break;
            }
        } finally {
            writable.drop();
        }
    })();
    return { readable, completion };
};
