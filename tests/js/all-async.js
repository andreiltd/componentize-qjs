export async function echoU32(x) { return x; }
export async function echoString(s) { return s; }
export async function echoBool(b) { return b; }
export async function doNothing() {}

export async function delayedEcho(x) {
    await Promise.resolve();
    return x + 1;
}

export async function echoPoint(p) { return { x: p.x * 2, y: p.y * 2 }; }
export async function echoOption(x) { return x; }

export async function safeDivide(a, b) {
    if (b === 0) {
        throw "division by zero";
    }
    return a / b;
}

export async function doubleList(xs) { return xs.map(x => x * 2); }

export async function chain(x) {
    let result = x;
    result = await Promise.resolve(result + 1);
    result = await Promise.resolve(result + 1);
    result = await Promise.resolve(result + 1);
    return result;
}

export async function validate(x) {
    if (x > 100) {
        throw undefined;
    }
    return x * 2;
}

export async function process(kind) {
    if (kind === 0) return { tag: "empty" };
    if (kind === 1) return { tag: "message", val: "hello" };
    return { tag: "code", val: 42 };
}

let count = 0;
export async function nextCount() {
    await Promise.resolve();
    return ++count;
}
