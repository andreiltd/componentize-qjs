export async function echoU8(v) { return v; }
export async function echoU16(v) { return v; }
export async function echoU32(v) { return v; }
export async function echoS32(v) { return v; }
export async function echoS64(v) { return v; }
export async function echoU64(v) { return v; }
export async function echoF64(v) { return v; }
export async function echoBool(v) { return v; }
export async function echoChar(v) { return v; }
export async function echoString(v) { return v; }
export async function concatStrings(a, b) { return a + b; }
export async function echoBytes(v) { return v; }
export async function echoListU32(v) { return v; }
export async function echoListString(v) { return v; }
export async function echoRecord(v) { return v; }
export async function echoTuple(v) { return v; }
export async function echoOptionString(v) { return v; }
export async function echoResult(v) { return v; }
export async function echoVariant(v) { return v; }
export async function echoEnum(v) { return v; }
export async function echoFlags(v) { return v; }

let accumulated = [];
export async function accumulate(v) {
    accumulated.push(v);
    return [...accumulated];
}
export async function resetAccumulator() {
    accumulated = [];
}

export async function getMemoryUsage() {
    return __cqjs.getMemoryUsage();
}
export async function runGc() {
    __cqjs.runGc();
}
