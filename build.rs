fn main() {
    // 编译 Slint UI 文件
    slint_build::compile("ui/main.slint").unwrap();
    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=ui/theme.slint");
    println!("cargo:rerun-if-changed=ui/components/item_card.slint");
    println!("cargo:rerun-if-changed=ui/components/classification_tab.slint");
}
