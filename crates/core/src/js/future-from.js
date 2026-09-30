wit.Future.from = function(value, type) {
    const { readable, writable } = wit.Future(type);
    const completion = (async () => {
        try {
            await writable.write(await value);
        } finally {
            writable.drop();
        }
    })();
    return { readable, completion };
};
