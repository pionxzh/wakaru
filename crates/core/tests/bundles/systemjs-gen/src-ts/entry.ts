import greet, { named } from "./dep";

export const value = named + 1;

export default function run(): string {
  return greet(String(value));
}
