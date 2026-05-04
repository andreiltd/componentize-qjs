import math from "local:test/math";

// Use the imported math interface
export function doubleAdd(a, b) {
    const sum = math.add(a, b);
    return math.multiply(sum, 2);
}
