import { __extends } from "tslib";
var Child = /** @class */ (function (_super) {
    __extends(Child, _super);
    function Child() {
        var _this = _super !== null && _super.apply(this, arguments) || this;
        _this.label = "main";
        _this.read = function () { return _this.label; };
        return _this;
    }
    Child.prototype.value = function () { return _super.prototype.value.call(this) + 1; };
    return Child;
}(Parent));
export { Child };
