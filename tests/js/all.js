export function greet() { return "Hello, World!"; }
export function add(a, b) { return a + b; }
export function addU8(a, b) { return a + b; }
export function addS8(a, b) { return a + b; }
export function addU16(a, b) { return a + b; }
export function addS16(a, b) { return a + b; }
export function addU32(a, b) { return a + b; }
export function addS32(a, b) { return a + b; }
export function addU64(a, b) { return a + b; }
export function addS64(a, b) { return a + b; }
export function addF32(a, b) { return a + b; }
export function addF64(a, b) { return a + b; }
export function negate(b) { return !b; }
export function addPoints(a, b) { return { x: a.x + b.x, y: a.y + b.y }; }
export function sumList(nums) { return nums.reduce((a, b) => a + b, 0); }

export function scaleMap(values) {
    if (!(values instanceof Map)) {
        throw new TypeError("expected Map");
    }
    const result = new Map();
    for (const [key, value] of values) {
        result.set(key.toUpperCase(), value * 2);
    }
    return result;
}

export function sumMap(values) {
    const result = new Map();
    for (const [key, items] of values) {
        result.set(key, items.reduce((sum, item) => sum + item, 0));
    }
    return result;
}

export function emptyMap() { return new Map(); }
export function bytes() { return new Uint8Array([0, 1, 127, 255]); }
export function empty() { return new Uint8Array(); }

export function maybeDouble(n) {
    if (n === null || n === undefined) { return null; }
    return n * 2;
}

export function safeDiv(a, b) {
    if (b === 0) { throw "division by zero"; }
    return Math.floor(a / b);
}

export function takeString(s) { return s.length; }
export function returnString() { return "hello from js"; }
export function concatStrings(a, b) { return a + b; }
export function takeChar(c) { return c.codePointAt(0); }
export function returnChar() { return "A"; }

export function identifyColor(c) {
    if (c === "red") return "is red";
    if (c === "green") return "is green";
    if (c === "blue") return "is blue";
    return "unknown";
}

export function favoriteColor() { return "green"; }

export function describeShape(s) {
    if (s.tag === "circle") return "circle with radius " + s.val;
    if (s.tag === "none") return "no shape";
    return "unknown";
}

export function makeCircle(r) { return { tag: "circle", val: r }; }
export function checkRead(p) { return p.read === true; }
export function readWrite() { return { read: true, write: true }; }
export function swap(a, b) { return [b, a]; }

export function sumTen(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10) {
    return a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10;
}

export function getAnswer() { return 42; }
export function getMessage() { return "hello"; }
export function getFlag() { return true; }
export function flatten(nested) { return nested.reduce((acc, arr) => acc.concat(arr), []); }
export function greetPerson(p) { return "Hello " + p.name + ", age " + p.age + ", active: " + p.active; }
export function makePerson(name, age) { return { name: name, age: age, active: true }; }
export function joinStrings(parts, sep) { return parts.join(sep); }
export function countStrings(parts) { return parts.length; }
export function hello() { return "hello"; }

export function deepFlatten(nested) {
    let result = [];
    for (const mid of nested) {
        for (const inner of mid) {
            for (const v of inner) {
                result.push(v);
            }
        }
    }
    return result;
}

let count = 0;
export function nextCount() { return ++count; }
