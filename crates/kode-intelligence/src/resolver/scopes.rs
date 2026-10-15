use super::*;
impl State<'_> {
    pub fn scope_chain(&self,scope:ScopeId)->Vec<ScopeId> {
        let mut result=vec![];let mut visited=BTreeSet::new();let mut current=Some(scope);
        while let Some(id)=current {if !visited.insert(id) {break;}result.push(id);current=self.scope(id).and_then(|s|s.parent_id);}
        result
    }
    pub fn owner_type(&self,scope:ScopeId)->Option<SymbolId> {self.scope_chain(scope).iter().filter_map(|id|self.scope(*id)).filter_map(|s|s.owner_symbol_id).find(|id|self.symbol(*id).is_some_and(|s|is_type(s.kind)))}
    pub fn owner_symbol(&self,scope:ScopeId)->Option<SymbolId> {self.scope_chain(scope).iter().filter_map(|id|self.scope(*id)).find_map(|s|s.owner_symbol_id)}
    pub fn package(&self,file:FileId)->Option<&str> {
        self.project.deltas.get(&file)?.symbols.iter().find(|s|s.kind==SymbolKind::Package).and_then(|s|s.qualified_name.as_deref())
    }
    pub fn members(&self,owner:SymbolId)->Vec<SymbolId> {
        let mut pending=vec![owner];let mut visited=BTreeSet::new();let mut names=BTreeSet::new();let mut result=vec![];
        while let Some(owner)=pending.pop() {
            if !visited.insert(owner) {continue;}
            let relations:Vec<_>=self.outgoing.get(&owner).into_iter().flatten().map(|i|&self.project.relations[*i]).collect();
            for relation in &relations {if relation.kind==RelationKind::Contains && let Some(s)=self.symbol(relation.to) && names.insert((s.name.as_str(),s.kind)) {result.push(s.id);}}
            pending.extend(relations.into_iter().rev().filter(|r|matches!(r.kind,RelationKind::Extends|RelationKind::Implements|RelationKind::AliasOf)).map(|r|r.to));
        }
        result
    }
    pub fn base_type(&self,owner:SymbolId)->Option<SymbolId> {self.outgoing.get(&owner).into_iter().flatten().map(|i|&self.project.relations[*i]).find(|r|matches!(r.kind,RelationKind::Extends|RelationKind::Implements)).map(|r|r.to)}
    pub fn inherits(&self,current:SymbolId,base:SymbolId)->bool {
        let mut pending=vec![current];let mut visited=BTreeSet::new();
        while let Some(owner)=pending.pop() {if !visited.insert(owner) {continue;}for relation in self.outgoing.get(&owner).into_iter().flatten().map(|i|&self.project.relations[*i]).filter(|r|matches!(r.kind,RelationKind::Extends|RelationKind::Implements)) {if relation.to==base {return true;}pending.push(relation.to);}}
        false
    }
    pub fn accessible(&self,symbol:&SymbolRecord,file:FileId,scope:ScopeId)->bool {
        match symbol.flags & SymbolFlags::VISIBILITY_MASK {
            0|SymbolFlags::PUBLIC|SymbolFlags::INTERNAL=>true,
            SymbolFlags::PRIVATE=>symbol.owner_symbol_id.map_or(symbol.file_id==file,|owner|self.owner_type(scope)==Some(owner)),
            SymbolFlags::PROTECTED=>symbol.owner_symbol_id.is_some_and(|owner|self.owner_type(scope).is_some_and(|current|current==owner || self.inherits(current,owner))),
            _=>true,
        }
    }
}
