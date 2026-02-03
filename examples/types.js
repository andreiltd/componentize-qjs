// Numeric types (using camelCase - runtime converts from WIT kebab-case)
function addU32(a, b) {
    return (a + b) >>> 0; // unsigned 32-bit
}

function addS32(a, b) {
    return (a + b) | 0; // signed 32-bit
}

function addF64(a, b) {
    return a + b;
}

function negate(b) {
    return !b;
}

function toUpper(c) {
    return c.toUpperCase();
}

// Record
function addPoints(a, b) {
    return { x: a.x + b.x, y: a.y + b.y };
}

// List
function sumList(nums) {
    return nums.reduce((acc, n) => acc + n, 0);
}

// Option - null/undefined = none, value = some
function maybeDouble(n) {
    if (n === null || n === undefined) {
        return null;
    }
    return n * 2;
}

// Result - {tag: "ok", val: ...} or {tag: "err", val: ...}
function safeDivide(a, b) {
    if (b === 0) {
        return { tag: "err", val: "division by zero" };
    }
    return { tag: "ok", val: Math.floor(a / b) };
}

// Enum - represented as discriminant number
function colorName(c) {
    const names = ["red", "green", "blue"];
    return names[c] || "unknown";
}

// Flags - represented as bitmask
function checkRead(p) {
    return (p & 1) !== 0; // read is bit 0
}

// Variant - {tag: discriminant, val: payload}
function shapeArea(s) {
    if (s.tag === 0) {
        // circle - val is radius
        const r = s.val;
        return Math.PI * r * r;
    } else {
        // rectangle - val is point {x, y} representing width/height
        return s.val.x * s.val.y;
    }
}
