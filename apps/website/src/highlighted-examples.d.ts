declare module "virtual:highlighted-examples" {
  type Token = { content: string; color?: string; fontStyle?: number };
  const examples: Record<string, Token[][] | undefined>;
  export default examples;
}
