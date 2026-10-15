//! Deterministic native resolution. Parsing and publication remain separate.
mod declared_types;
mod imports;
mod names;
mod relations;
mod scopes;
use crate::{ids::*,model::*};
use kode_core::{ByteOffset,DomainError,Result};
use kode_platform::jobs::CancellationToken;
use std::{collections::{BTreeMap,BTreeSet},time::{Duration,Instant}};

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct UnresolvedName {
    pub file_id:FileId,pub scope_id:ScopeId,pub name:String,pub range:SourceRange,pub kind:OccurrenceKind,
}
pub struct ResolutionReport {pub project:ResolvedSemanticProject,pub unresolved:Vec<UnresolvedName>}
pub struct SemanticResolver;
impl SemanticResolver {
    pub fn resolve(deltas:impl IntoIterator<Item=FileSemanticDelta>,cancel:&CancellationToken)->Result<ResolutionReport> {
        let mut state=State::new(deltas,cancel)?;
        state.resolve_declared_types()?;
        state.resolve_occurrences()?;
        state.derive_dependencies()?;
        state.finish()
    }
}
pub(super) struct State<'a> {
    pub project:ResolvedSemanticProject,
    pub symbols:BTreeMap<SymbolId,usize>,
    pub scopes:BTreeMap<ScopeId,usize>,
    pub by_name:BTreeMap<String,Vec<SymbolId>>,
    pub qualified:BTreeMap<String,Vec<SymbolId>>,
    pub scoped:BTreeMap<ScopeId,Vec<SymbolId>>,
    pub outgoing:BTreeMap<SymbolId,Vec<usize>>,
    pub cancel:&'a CancellationToken,
    started:Instant,
    operations:usize,
}
impl<'a> State<'a> {
    fn new(deltas:impl IntoIterator<Item=FileSemanticDelta>,cancel:&'a CancellationToken)->Result<Self> {
        let mut deltas:Vec<_>=deltas.into_iter().collect();deltas.sort_by_key(|d|d.file.id);
        let mut project=ResolvedSemanticProject::from_deltas(deltas)?;
        project.symbols.sort_by_key(|s|s.id);project.scopes.sort_by_key(|s|s.id);project.occurrences.sort_by_key(|o|(o.file_id,o.range.start,o.range.end));
        let symbols=project.symbols.iter().enumerate().map(|(i,s)|(s.id,i)).collect();let scopes=project.scopes.iter().enumerate().map(|(i,s)|(s.id,i)).collect();
        let mut state=Self {project,symbols,scopes,by_name:BTreeMap::new(),qualified:BTreeMap::new(),scoped:BTreeMap::new(),outgoing:BTreeMap::new(),cancel,started:Instant::now(),operations:0};
        for symbol in &state.project.symbols {
            state.by_name.entry(symbol.name.clone()).or_default().push(symbol.id);
            if let Some(name)=&symbol.qualified_name {state.qualified.entry(name.clone()).or_default().push(symbol.id);}
            state.scoped.entry(symbol.scope_id).or_default().push(symbol.id);
        }
        for (i,relation) in state.project.relations.iter().enumerate() {state.outgoing.entry(relation.from).or_default().push(i);}
        state.check()?;Ok(state)
    }
    pub fn check(&mut self)->Result<()> {
        self.operations+=1;
        if self.cancel.is_cancelled() {return Err(DomainError::Cancelled);}
        if self.operations>2_000_000 || self.started.elapsed()>Duration::from_secs(5) {return Err(DomainError::Unsupported("semantic resolution work limit exceeded".into()));}
        Ok(())
    }
    pub fn symbol(&self,id:SymbolId)->Option<&SymbolRecord> {self.symbols.get(&id).map(|i|&self.project.symbols[*i])}
    pub fn scope(&self,id:ScopeId)->Option<&ScopeRecord> {self.scopes.get(&id).map(|i|&self.project.scopes[*i])}
    pub fn language(&self,file:FileId)->&str {self.project.deltas.get(&file).map_or("",|d|d.file.language_id.as_str())}
    pub fn is_global(&self,symbol:&SymbolRecord)->bool {self.scope(symbol.scope_id).is_some_and(|s|matches!(s.kind,ScopeKind::File|ScopeKind::Package|ScopeKind::Module|ScopeKind::Namespace))}
    pub fn relation(&mut self,from:SymbolId,to:SymbolId,kind:RelationKind,confidence:Confidence) {
        if self.outgoing.get(&from).into_iter().flatten().any(|i| {let r=&self.project.relations[*i];r.to==to && r.kind==kind}) {return;}
        self.outgoing.entry(from).or_default().push(self.project.relations.len());self.project.relations.push(RelationRecord {from,to,kind,confidence});
    }
    fn finish(mut self)->Result<ResolutionReport> {
        self.check()?;
        let mut unresolved=vec![];
        for occurrence in &self.project.occurrences {
            if occurrence.resolved_symbol_id.is_none() && !declared_types::is_primitive(&occurrence.text,self.language(occurrence.file_id)) {
                unresolved.push(UnresolvedName {file_id:occurrence.file_id,scope_id:occurrence.scope_id,name:occurrence.text.clone(),range:occurrence.range,kind:occurrence.kind});
            }
        }
        self.project.relations.sort_by_key(|r|(r.from,r.to,r.kind));
        self.project.dependencies.sort_by_key(|d|(d.from_file_id,d.to_file_id,d.kind));
        // Validate the enriched representation using exactly the query/store gate.
        let project=crate::snapshot::SemanticSnapshot::new(0,self.project)?.project().clone();
        Ok(ResolutionReport {project,unresolved})
    }
    fn resolve_occurrences(&mut self)->Result<()> {
        let mut resolved=BTreeMap::<OccurrenceId,OccurrenceRecord>::new();
        for mut occurrence in self.project.occurrences.clone() {
            self.check()?;
            let target=occurrence.resolved_symbol_id.or_else(|| {
                if occurrence.text=="this" {self.owner_type(occurrence.scope_id)}
                else if occurrence.text=="super" {self.owner_type(occurrence.scope_id).and_then(|owner|self.base_type(owner))}
                else if occurrence.kind==OccurrenceKind::MemberReference {
                    let receiver=occurrence.receiver_occurrence_id.and_then(|id|resolved.get(&id));
                    let symbol=receiver.and_then(|o|o.resolved_symbol_id).and_then(|id|self.symbol(id));
                    let ty=symbol.and_then(|s|self.occurrence_type(receiver.expect("receiver accompanies symbol"),s));
                    ty.as_ref().and_then(primary_named).and_then(|owner|self.members(owner).into_iter().find(|id|self.symbol(*id).is_some_and(|s|s.name==occurrence.text && self.accessible(s,occurrence.file_id,occurrence.scope_id))))
                } else if occurrence.kind==OccurrenceKind::Import {
                    self.import_occurrence(&occurrence)
                } else {
                    let kind=match occurrence.kind {OccurrenceKind::TypeReference=>NameKind::Type,OccurrenceKind::Call=>NameKind::Callable,_=>NameKind::Any};
                    self.find_name(&occurrence.text,occurrence.scope_id,occurrence.file_id,occurrence.range.start,kind,None)
                }
            });
            if let Some(id)=target {
                occurrence.resolved_symbol_id=Some(id);occurrence.confidence=Confidence::inferred(0.95)?;
                if occurrence.kind==OccurrenceKind::Call && let Some(owner)=self.owner_symbol(occurrence.scope_id) {self.relation(owner,id,RelationKind::Calls,Confidence::inferred(0.95)?);}
            }
            resolved.insert(occurrence.id,occurrence);
        }
        self.project.occurrences=resolved.into_values().collect();self.project.occurrences.sort_by_key(|o|(o.file_id,o.range.start,o.range.end));Ok(())
    }
    pub fn occurrence_type(&self,occurrence:&OccurrenceRecord,symbol:&SymbolRecord)->Option<TypeRef> {
        if is_type(symbol.kind) && (matches!(occurrence.kind,OccurrenceKind::Call|OccurrenceKind::TypeReference) || matches!(occurrence.text.as_str(),"this"|"super")) {return Some(TypeRef::named(symbol.id));}
        symbol.declared_type_id.or(symbol.inferred_type_id).and_then(|id|self.project.types.get(&id)).map(|t|t.r#ref.clone())
    }
}
#[derive(Clone,Copy)]
pub(super) enum NameKind {Any,Type,Callable,Tag(SymbolKind)}
pub(super) fn is_type(kind:SymbolKind)->bool {matches!(kind,SymbolKind::Class|SymbolKind::Interface|SymbolKind::Struct|SymbolKind::Enum|SymbolKind::TypeAlias)}
pub(super) fn primary_named(ty:&TypeRef)->Option<SymbolId> {match ty {TypeRef::Named {symbol_id}=>Some(*symbol_id),TypeRef::Generic {base,..}|TypeRef::Nullable {inner:base}=>primary_named(base),_=>None}}
