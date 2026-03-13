async function echoU8(v) { return v; }
async function echoU16(v) { return v; }
async function echoU32(v) { return v; }
async function echoS32(v) { return v; }
async function echoS64(v) { return v; }
async function echoU64(v) { return v; }
async function echoF64(v) { return v; }
async function echoBool(v) { return v; }
async function echoChar(v) { return v; }
async function echoString(v) { return v; }
async function concatStrings(a, b) { return a + b; }
async function echoBytes(v) { return v; }
async function echoListU32(v) { return v; }
async function echoListString(v) { return v; }
async function echoRecord(v) { return v; }
async function echoTuple(v) { return v; }
async function echoOptionString(v) { return v; }
async function echoResult(v) { return v; }
async function echoVariant(v) { return v; }
async function echoEnum(v) { return v; }
async function echoFlags(v) { return v; }

let accumulated = [];
async function accumulate(v) {
    accumulated.push(v);
    return [...accumulated];
}
async function resetAccumulator() {
    accumulated = [];
}

async function getMemoryUsage() {
    return __cqjs.getMemoryUsage();
}
async function runGc() {
    __cqjs.runGc();
}
