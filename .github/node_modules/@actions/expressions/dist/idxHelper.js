export class idxHelper {
    constructor(star, idx) {
        this.star = star;
        if (!idx) {
            return;
        }
        if (!star) {
            if (idx.primitive) {
                this.str = idx.coerceString();
            }
            let f = idx.number();
            if (!isNaN(f) && isFinite(f) && f >= 0) {
                f = Math.floor(f);
                this.int = f;
            }
        }
    }
}
//# sourceMappingURL=idxHelper.js.map