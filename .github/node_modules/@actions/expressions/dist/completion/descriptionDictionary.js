import { Dictionary } from "../data/dictionary.js";
import { Kind } from "../data/expressiondata.js";
export function isDescriptionDictionary(x) {
    return x.kind === Kind.Dictionary && x instanceof DescriptionDictionary;
}
export class DescriptionDictionary extends Dictionary {
    constructor(...pairs) {
        super();
        this.descriptions = new Map();
        this.complete = true;
        for (const p of pairs) {
            this.add(p.key, p.value, p.description);
        }
    }
    add(key, value, description) {
        if (this.get(key) !== undefined) {
            // Key already added, ignore
            return;
        }
        super.add(key, value);
        if (description) {
            this.descriptions.set(key, description);
        }
    }
    pairs() {
        const pairs = super.pairs();
        return pairs.map(p => ({ ...p, description: this.descriptions.get(p.key) }));
    }
    getDescription(key) {
        return this.descriptions.get(key);
    }
}
//# sourceMappingURL=descriptionDictionary.js.map