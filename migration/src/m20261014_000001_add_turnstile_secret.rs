use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::text_null;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppSettings::Table)
                    .add_column(text_null(AppSettings::TurnstileSecretCiphertext))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppSettings::Table)
                    .drop_column(AppSettings::TurnstileSecretCiphertext)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AppSettings {
    Table,
    TurnstileSecretCiphertext,
}
