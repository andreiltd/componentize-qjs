export function echoU8(v) { return v; }
export function echoU16(v) { return v; }
export function echoU32(v) { return v; }
export function echoS32(v) { return v; }
export function echoS64(v) { return v; }
export function echoU64(v) { return v; }
export function echoF64(v) { return v; }
export function echoBool(v) { return v; }
export function echoChar(v) { return v; }
export function echoString(v) { return v; }
export function concatStrings(a, b) { return a + b; }
export function echoBytes(v) { return v; }
export function echoListU32(v) { return v; }
export function echoListString(v) { return v; }
export function echoRecord(v) { return v; }
export function echoTuple(v) { return v; }
export function echoOptionString(v) { return v; }
export function echoResult(v) { return v; }
export function echoVariant(v) { return v; }
export function echoEnum(v) { return v; }
export function echoFlags(v) { return v; }

let accumulated = [];
export function accumulate(v) {
    accumulated.push(v);
    return [...accumulated];
}
export function resetAccumulator() {
    accumulated = [];
}

export function getMemoryUsage() {
    return __cqjs.getMemoryUsage();
}
export function runGc() {
    __cqjs.runGc();
}
