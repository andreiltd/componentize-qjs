function echoU8(v) { return v; }
function echoU16(v) { return v; }
function echoU32(v) { return v; }
function echoS32(v) { return v; }
function echoS64(v) { return v; }
function echoU64(v) { return v; }
function echoF64(v) { return v; }
function echoBool(v) { return v; }
function echoChar(v) { return v; }
function echoString(v) { return v; }
function concatStrings(a, b) { return a + b; }
function echoBytes(v) { return v; }
function echoListU32(v) { return v; }
function echoListString(v) { return v; }
function echoRecord(v) { return v; }
function echoTuple(v) { return v; }
function echoOptionString(v) { return v; }
function echoResult(v) { return v; }
function echoVariant(v) { return v; }
function echoEnum(v) { return v; }
function echoFlags(v) { return v; }

let accumulated = [];
function accumulate(v) {
    accumulated.push(v);
    return [...accumulated];
}
function resetAccumulator() {
    accumulated = [];
}

function getMemoryUsage() {
    return __cqjs.getMemoryUsage();
}
function runGc() {
    __cqjs.runGc();
}
