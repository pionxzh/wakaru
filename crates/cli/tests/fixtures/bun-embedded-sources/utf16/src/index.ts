/*! © Example Corp — legal notice */
import note from "./note.txt" with { type: "text" };
import { addPair, type Pair } from "./util/math";
const pair: Pair = { left: 2, right: 3 };
console.log(note.trim(), addPair(pair));
