import { Kind } from "./expressiondata.js";
export class BooleanData {
    constructor(value) {
        this.value = value;
        this.kind = Kind.Boolean;
        this.primitive = true;
    }
    coerceString() {
        if (this.value) {
            return "true";
        }
        return "false";
    }
    number() {
        if (this.value) {
            return 1;
        }
        return 0;
    }
}
//# sourceMappingURL=boolean.js.map