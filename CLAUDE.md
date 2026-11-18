# CLAUDE.md

## Language: ASD-STE100 Simplified Technical English

Write all prose in ASD-STE100 Simplified Technical English (STE). This applies to:

- Documentation (`README.md`, `docs/`)
- Code comments and doc comments
- Commit messages and pull request descriptions
- Text that the user sees in the application (labels, help text, errors)

Do not apply STE to code identifiers, or to text that quotes an external source.

### Words

- Use approved STE words with their approved meaning only. One word has one meaning; one meaning has one word.
- Use technical names (for example `Segment`, `Session`, crate names, CLI flags) and technical verbs (for example "parse", "render", "compile") as necessary. Use the same technical name for the same thing every time.
- Use only these verb forms: infinitive, imperative, simple present, simple past, simple future, and the past participle as an adjective.
- Do not use the "-ing" form of a verb, except in a technical name.
- Do not use phrasal verbs when a single verb has the same meaning ("set up" → "configure", "find out" → "find").

### Sentences

- Procedural sentences (instructions): 20 words or fewer.
- Descriptive sentences: 25 words or fewer.
- Write one instruction in each sentence. Put more than one instruction in one sentence only when the actions occur at the same time.
- Use the active voice. Use the passive voice only in descriptive text when the agent is not known or not important.
- Write instructions in the imperative ("Run the tests.", not "You should run the tests.").
- Use articles ("a", "an", "the") and demonstratives ("this", "these") where possible. Do not omit them to make text shorter.
- Do not use more than three nouns in a noun cluster. Rewrite long clusters with prepositions.

### Paragraphs

- Write one topic in each paragraph.
- Descriptive paragraphs: six sentences or fewer.
- Put a warning or caution before the instruction that it applies to.

### Commit messages

- Write the subject line as an imperative sentence that tells the result for the user (for example "Keep the Segments column readable as the terminal narrows").
