import type { Producer } from "../lib/roundTrip";

export type ProducerWorkerRequest = {
  type: "compile";
  id: number;
  producer: Producer;
  source: string;
};

export type ProducerWorkerResponse =
  | { type: "compile-result"; id: number; code: string }
  | { type: "compile-error"; id: number; error: string };
