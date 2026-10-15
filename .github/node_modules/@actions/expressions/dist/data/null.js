import { Kind } from "./expressiondata.js";
export class Null {
    constructor() {
        this.kind = Kind.Null;
        this.primitive = true;
    }
    coerceString() {
        return "";
    }
    number() {
        return 0;
    }
}
//# sourceMappingURL=null.js.map