import { Kind, kindStr } from "./expressiondata.js";
export class Dictionary {
    constructor(...pairs) {
        this.keys = [];
        this.v = [];
        this.indexMap = {};
        this.kind = Kind.Dictionary;
        this.primitive = false;
        for (const p of pairs) {
            this.add(p.key, p.value);
        }
    }
    coerceString() {
        return kindStr(this.kind);
    }
    number() {
        return NaN;
    }
    add(key, value) {
        if (key.toLowerCase() in this.indexMap) {
            return;
        }
        this.keys.push(key);
        this.v.push(value);
        this.indexMap[key.toLowerCase()] = this.v.length - 1;
    }
    get(key) {
        const index = this.indexMap[key.toLowerCase()];
        if (index === undefined) {
            return undefined;
        }
        return this.v[index];
    }
    values() {
        return this.v;
    }
    pairs() {
        const result = [];
        for (const key of this.keys) {
            result.push({ key, value: this.v[this.indexMap[key.toLowerCase()]] });
        }
        return result;
    }
}
export function isDictionary(x) {
    return x.kind === Kind.Dictionary;
}
//# sourceMappingURL=dictionary.js.map