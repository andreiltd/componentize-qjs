// Use the imported math interface
// The import will be available as "local:test/math"
function doubleAdd(a, b) {
    // Access the math import interface
    const math = globalThis["local:test/math"];
    const sum = math.add(a, b);
    return math.multiply(sum, 2);
}
