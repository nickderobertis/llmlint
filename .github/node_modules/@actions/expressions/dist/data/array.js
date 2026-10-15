import { Kind, kindStr } from "./expressiondata.js";
export class Array {
    constructor(...data) {
        this.v = [];
        this.kind = Kind.Array;
        this.primitive = false;
        for (const d of data) {
            this.add(d);
        }
    }
    coerceString() {
        return kindStr(this.kind);
    }
    number() {
        return NaN;
    }
    add(value) {
        this.v.push(value);
    }
    get(index) {
        return this.v[index];
    }
    values() {
        return this.v;
    }
}
//# sourceMappingURL=array.js.map