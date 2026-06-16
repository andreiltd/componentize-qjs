// Numeric types (using camelCase - runtime converts from WIT kebab-case)
export function addU32(a, b) {
    return (a + b) >>> 0; // unsigned 32-bit
}

export function addS32(a, b) {
    return (a + b) | 0; // signed 32-bit
}

export function addF64(a, b) {
    return a + b;
}

export function negate(b) {
    return !b;
}

export function toUpper(c) {
    return c.toUpperCase();
}

// Record
export function addPoints(a, b) {
    return { x: a.x + b.x, y: a.y + b.y };
}

// List
export function sumList(nums) {
    return nums.reduce((acc, n) => acc + n, 0);
}

// Option - null/undefined = none, value = some
export function maybeDouble(n) {
    if (n === null || n === undefined) {
        return null;
    }
    return n * 2;
}

// Top-level result returns use the JS exception convention:
// return the ok payload, or throw the err payload.
export function safeDivide(a, b) {
    if (b === 0) {
        throw "division by zero";
    }
    return Math.floor(a / b);
}

// Enum - represented as its case-name string
export function colorName(c) {
    return c;
}

// Flags - represented as a { name: boolean } object
export function checkRead(p) {
    return p.read === true; // read flag
}

// Variant - { tag: case-name, val: payload }
export function shapeArea(s) {
    if (s.tag === "circle") {
        // circle - val is radius
        const r = s.val;
        return Math.PI * r * r;
    } else {
        // rectangle - val is point {x, y} representing width/height
        return s.val.x * s.val.y;
    }
}
