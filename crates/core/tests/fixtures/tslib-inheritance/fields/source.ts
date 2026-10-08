export class Child extends Parent {
    label = "main";
    read = () => this.label;
    value() { return super.value() + 1; }
}
