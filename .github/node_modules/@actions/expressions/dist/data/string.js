import { Kind } from "./expressiondata.js";
export class StringData {
    constructor(value) {
        this.value = value;
        this.kind = Kind.String;
        this.primitive = true;
    }
    coerceString() {
        return this.value;
    }
    number() {
        return Number(this.value);
    }
}
//# sourceMappingURL=string.js.map