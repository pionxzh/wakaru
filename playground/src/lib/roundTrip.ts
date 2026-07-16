export type PlaygroundMode = "decompile" | "roundtrip";
export type Producer = "babel" | "swc" | "esbuild";

export interface ProducerDescriptor {
  value: Producer;
  label: string;
  recipe: string;
}

export const PRODUCERS: ProducerDescriptor[] = [
  {
    value: "babel",
    label: "Babel",
    recipe: "Babel 8.0.4 · preset-env for Chrome 58 · classic JSX",
  },
  {
    value: "swc",
    label: "SWC",
    recipe: "SWC 1.15.46 · ES2017 target · classic JSX",
  },
  {
    value: "esbuild",
    label: "esbuild",
    recipe: "esbuild 0.28 · ES2017 target · classic JSX",
  },
];

export const ROUND_TRIP_EXAMPLE = `\
export function UserCard({ user, theme = "dark" }) {
  const tags = [...(user?.tags ?? []), "active"];

  return (
    <article className={\`card card--\${theme}\`}>
      <h2>{user?.profile?.name ?? "Anonymous"}</h2>
      <ul>
        {tags.map((tag) => <li key={tag}>{tag}</li>)}
      </ul>
    </article>
  );
}
`;

export function isPlaygroundMode(value: unknown): value is PlaygroundMode {
  return value === "decompile" || value === "roundtrip";
}

export function isProducer(value: unknown): value is Producer {
  return value === "babel" || value === "swc" || value === "esbuild";
}

export function getProducerDescriptor(producer: Producer): ProducerDescriptor {
  return PRODUCERS.find(({ value }) => value === producer) ?? PRODUCERS[0];
}
