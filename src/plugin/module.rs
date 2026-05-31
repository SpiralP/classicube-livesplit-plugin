#[cfg(test)]
mod tests;

pub trait Module {
    fn free(&mut self) {}
    fn reset(&mut self) {}
    fn on_new_map(&mut self) {}
    fn on_new_map_loaded(&mut self) {}

    fn children(&mut self) -> Vec<&mut dyn Module> {
        vec![]
    }

    fn children_call_order(&mut self) -> Vec<&mut dyn Module> {
        let mut children = self.children();
        children.reverse();
        children
    }

    fn handle_free(&mut self) {
        for child in self.children_call_order() {
            child.handle_free();
        }
        self.free();
    }
    fn handle_reset(&mut self) {
        for child in self.children_call_order() {
            child.handle_reset();
        }
        self.reset();
    }
    fn handle_on_new_map(&mut self) {
        for child in self.children_call_order() {
            child.handle_on_new_map();
        }
        self.on_new_map();
    }
    fn handle_on_new_map_loaded(&mut self) {
        for child in self.children_call_order() {
            child.handle_on_new_map_loaded();
        }
        self.on_new_map_loaded();
    }
}
