// Test calling WASI imports
function getRandomU64() {
    // Access the WASI random interface
    const random = globalThis["wasi:random/random@0.2.6"] || globalThis["wasi:random/random"];
    if (!random) {
        throw new Error("wasi:random/random not found");
    }
    return random.getRandomU64();
}
