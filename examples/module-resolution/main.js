import { offset } from "./config";
import { add } from "./lib/math.js";
import { prefix } from "local-greeter";

export function addWithOffset(a, b) {
    return add(a, b) + offset;
}

export function greet(name) {
    return `${prefix}, ${name}!`;
}
